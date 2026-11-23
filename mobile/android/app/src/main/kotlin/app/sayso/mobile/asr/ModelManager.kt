package app.sayso.mobile.asr

import android.app.ActivityManager
import android.content.Context
import app.sayso.mobile.settings.VoiceSettingsRepository
import app.sayso.mobile.settings.VoiceSettingsStore
import java.io.File
import java.io.IOException
import java.io.InputStream
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest

/** Opens [url] from byte [offset]. */
fun interface ModelFetcher {
    fun open(url: String, offset: Long): FetchResponse
}

/**
 * [resumed] is true when the body starts at the requested offset (HTTP 206);
 * false when the server sent the whole file, so a partial copy must be dropped.
 */
class FetchResponse(val body: InputStream, val resumed: Boolean)

/** Plain HTTPS with a Range header. Follows the redirects of the model host. */
object HttpModelFetcher : ModelFetcher {
    override fun open(url: String, offset: Long): FetchResponse {
        val conn = URL(url).openConnection() as HttpURLConnection
        conn.connectTimeout = 15_000
        conn.readTimeout = 30_000
        if (offset > 0) conn.setRequestProperty("Range", "bytes=$offset-")
        return when (val code = conn.responseCode) {
            HttpURLConnection.HTTP_PARTIAL -> FetchResponse(conn.inputStream, resumed = true)
            HttpURLConnection.HTTP_OK -> FetchResponse(conn.inputStream, resumed = false)
            else -> {
                conn.disconnect()
                throw IOException("HTTP $code for $url")
            }
        }
    }
}

sealed interface ModelState {
    data object NotInstalled : ModelState
    data class Partial(val downloadedBytes: Long, val totalBytes: Long) : ModelState
    data object Installed : ModelState
}

/** A model needs more RAM than the device has; the app shows it as a warning, not a block. */
data class RamAdvisory(val model: AsrModel, val deviceRamBytes: Long)

class ModelDownloadCancelled : IOException("download cancelled")

/**
 * Downloads, checks, deletes, and activates the models of [ModelCatalog].
 *
 * Each model has its own folder under [root]. A file downloads to `<name>.part`,
 * resumes from there after an interruption, and gets its real name only after
 * its size and sha256 match the catalog. The model counts as installed when
 * every file is present at its catalog size and the `.installed` marker is
 * written last.
 *
 * [download] blocks: call it from a background thread. The companion app owns
 * when to call it; the keyboard only reads [isInstalled] and [modelDir].
 */
class ModelManager(
    private val root: File,
    private val settings: VoiceSettingsRepository,
    private val fetcher: ModelFetcher = HttpModelFetcher,
    val catalog: List<AsrModel> = ModelCatalog.ALL,
) {
    fun modelDir(model: AsrModel) = File(root, model.id)

    fun isInstalled(model: AsrModel): Boolean {
        val dir = modelDir(model)
        return File(dir, MARKER).isFile && model.files.all { File(dir, it.name).length() == it.sizeBytes }
    }

    fun state(model: AsrModel): ModelState {
        if (isInstalled(model)) return ModelState.Installed
        val dir = modelDir(model)
        val done = model.files.sumOf { f ->
            val whole = File(dir, f.name)
            if (whole.length() == f.sizeBytes) f.sizeBytes else File(dir, f.name + PART).length()
        }
        return if (done == 0L) ModelState.NotInstalled else ModelState.Partial(done, model.downloadBytes)
    }

    /**
     * Downloads every missing file of [model]. [onProgress] gets the bytes on
     * disk and the total. Throws [ModelDownloadCancelled] when [isCancelled]
     * turns true (the partial files stay, so the next call resumes), and
     * IOException on a network error or a file that fails its check (that
     * file's partial copy is deleted).
     */
    fun download(
        model: AsrModel,
        onProgress: (downloaded: Long, total: Long) -> Unit = { _, _ -> },
        isCancelled: () -> Boolean = { false },
    ) {
        val dir = modelDir(model)
        if (!dir.isDirectory && !dir.mkdirs()) throw IOException("cannot create $dir")
        File(dir, MARKER).delete()
        val total = model.downloadBytes
        var before = 0L
        for (file in model.files) {
            val target = File(dir, file.name)
            if (target.length() != file.sizeBytes) {
                target.delete()
                downloadFile(file, File(dir, file.name + PART), target, isCancelled) { onProgress(before + it, total) }
            }
            before += file.sizeBytes
            onProgress(before, total)
        }
        File(dir, MARKER).writeText(model.id)
    }

    private fun downloadFile(file: ModelFile, part: File, target: File, isCancelled: () -> Boolean, onBytes: (Long) -> Unit) {
        var offset = part.length()
        if (offset > file.sizeBytes) {
            part.delete()
            offset = 0
        }
        if (offset < file.sizeBytes) {
            val response = fetcher.open(file.url, offset)
            if (!response.resumed) offset = 0
            response.body.use { input ->
                java.io.FileOutputStream(part, offset > 0).use { out ->
                    val buf = ByteArray(BUFFER)
                    var written = offset
                    while (true) {
                        if (isCancelled()) throw ModelDownloadCancelled()
                        val n = input.read(buf)
                        if (n < 0) break
                        written += n
                        if (written > file.sizeBytes) {
                            out.close()
                            part.delete()
                            throw IOException("${file.name} is larger than ${file.sizeBytes} bytes")
                        }
                        out.write(buf, 0, n)
                        onBytes(written)
                    }
                }
            }
        }
        if (part.length() != file.sizeBytes) {
            throw IOException("${file.name}: got ${part.length()} of ${file.sizeBytes} bytes")
        }
        val sum = sha256(part)
        if (sum != file.sha256) {
            part.delete()
            throw IOException("${file.name} failed its check: sha256 $sum, expected ${file.sha256}")
        }
        if (!part.renameTo(target)) throw IOException("cannot rename $part")
    }

    /** Deletes the model's files, finished or partial, and drops it from the active models. */
    fun delete(model: AsrModel) {
        modelDir(model).deleteRecursively()
        val s = settings.load()
        val updated = s.withoutModel(model.id)
        if (updated != s) settings.save(updated)
    }

    /** Makes an installed [model] the active model of its kind. False when it is not installed. */
    fun setActive(model: AsrModel): Boolean {
        if (!isInstalled(model)) return false
        settings.save(settings.load().withActiveModel(model))
        return true
    }

    /** The active models that are installed: the final-text model and the live-text model. */
    fun activeModels(): ActiveModels {
        val s = settings.load()
        fun installed(id: String?) = catalog.firstOrNull { it.id == id }?.takeIf(::isInstalled)
        return ActiveModels(final = installed(s.finalModelId), live = installed(s.liveModelId))
    }

    fun ramAdvisory(model: AsrModel, deviceTotalRamBytes: Long): RamAdvisory? =
        if (needsMoreRam(model, deviceTotalRamBytes)) RamAdvisory(model, deviceTotalRamBytes) else null

    companion object {
        private const val PART = ".part"
        private const val MARKER = ".installed"
        private const val BUFFER = 64 * 1024

        /**
         * totalMem leaves out memory the kernel and firmware keep, so a phone sold
         * with 6 GB reports about 5.5 GB. Allow 15% under the nominal figure.
         */
        fun needsMoreRam(model: AsrModel, deviceTotalRamBytes: Long): Boolean =
            model.recommendedRamBytes > 0 && deviceTotalRamBytes < model.recommendedRamBytes * 85 / 100

        fun sha256(file: File): String {
            val digest = MessageDigest.getInstance("SHA-256")
            file.inputStream().use { input ->
                val buf = ByteArray(BUFFER)
                while (true) {
                    val n = input.read(buf)
                    if (n < 0) break
                    digest.update(buf, 0, n)
                }
            }
            return digest.digest().joinToString("") { "%02x".format(it) }
        }

        /** The manager for this app's private model folder, shared by the keyboard and the companion app. */
        fun forApp(context: Context): ModelManager =
            ModelManager(File(context.applicationContext.noBackupFilesDir, "asr-models"), VoiceSettingsStore(context))

        fun deviceTotalRam(context: Context): Long {
            val info = ActivityManager.MemoryInfo()
            context.getSystemService(ActivityManager::class.java).getMemoryInfo(info)
            return info.totalMem
        }
    }
}

/** What the :asr process loads for a dictation. Either may be null. */
data class ActiveModels(val final: AsrModel?, val live: AsrModel?) {
    /** True when there is a local model to give the final text. */
    val any: Boolean get() = final != null || live != null
}
