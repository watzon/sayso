package app.sayso.mobile.ui

import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import androidx.compose.runtime.mutableStateMapOf
import app.sayso.mobile.asr.AsrModel
import app.sayso.mobile.asr.ModelDownloadCancelled
import app.sayso.mobile.asr.ModelManager
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Model downloads for the whole process, so one keeps going when the Voice
 * tab is left, the screen rotates, or the activity is recreated. Each runs
 * [ModelManager.download] on its own thread; [status] is Compose state,
 * written only on the main thread. A download stops if the process dies;
 * the next one resumes from the partial files.
 */
object ModelDownloads {
    /** By model id: running downloads, and the last failure until the next try. */
    val status = mutableStateMapOf<String, DownloadStatus>()

    private val cancels = ConcurrentHashMap<String, AtomicBoolean>()
    private val main = Handler(Looper.getMainLooper())

    /** Starts [model] unless it is already downloading. [onInstalled] runs on the main thread when it finishes. */
    fun start(manager: ModelManager, model: AsrModel, onInstalled: () -> Unit) {
        val cancel = AtomicBoolean(false)
        if (cancels.putIfAbsent(model.id, cancel) != null) return
        status[model.id] = DownloadStatus.Running(0, model.downloadBytes)
        Thread({
            var lastPost = 0L
            val outcome: DownloadStatus? = try {
                manager.download(
                    model,
                    onProgress = { done, total ->
                        val now = SystemClock.uptimeMillis()
                        if (now - lastPost >= PROGRESS_EVERY_MS) {
                            lastPost = now
                            main.post { if (cancels[model.id] === cancel) status[model.id] = DownloadStatus.Running(done, total) }
                        }
                    },
                    isCancelled = cancel::get,
                )
                null
            } catch (_: ModelDownloadCancelled) {
                null
            } catch (e: Exception) {
                DownloadStatus.Failed(e.message ?: e.javaClass.simpleName)
            }
            main.post {
                cancels.remove(model.id, cancel)
                if (outcome == null) status.remove(model.id) else status[model.id] = outcome
                if (outcome == null && !cancel.get()) onInstalled()
            }
        }, "sayso-model-${model.id}").start()
    }

    /** Stops [model]'s download; its partial files stay so the next start resumes. */
    fun cancel(model: AsrModel) {
        cancels[model.id]?.set(true)
    }

    private const val PROGRESS_EVERY_MS = 200L
}
