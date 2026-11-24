package app.sayso.mobile.asr

import app.sayso.mobile.settings.VoiceSettings
import app.sayso.mobile.settings.VoiceSettingsRepository
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.File
import java.io.IOException
import java.io.InputStream
import java.nio.file.Files
import java.security.MessageDigest

private class MemorySettings(var value: VoiceSettings = VoiceSettings()) : VoiceSettingsRepository {
    override fun load() = value
    override fun save(settings: VoiceSettings) {
        value = settings
    }
}

/** Serves files from memory, honoring Range unless [ignoreRange]; can fail a body after [failAfter] bytes. */
private class FakeFetcher(private val files: Map<String, ByteArray>) : ModelFetcher {
    var ignoreRange = false
    var failAfter: Int? = null
    val offsets = mutableListOf<Long>()

    override fun open(url: String, offset: Long): FetchResponse {
        offsets += offset
        val bytes = files.getValue(url)
        val start = if (ignoreRange) 0 else offset.toInt()
        val body: InputStream = ByteArrayInputStream(bytes, start, bytes.size - start)
        val limit = failAfter ?: return FetchResponse(body, resumed = !ignoreRange && offset > 0)
        failAfter = null
        return FetchResponse(object : InputStream() {
            var sent = 0
            override fun read(): Int {
                if (sent >= limit) throw IOException("connection reset")
                sent++
                return body.read()
            }
        }, resumed = !ignoreRange && offset > 0)
    }
}

private fun sha(b: ByteArray) = MessageDigest.getInstance("SHA-256").digest(b).joinToString("") { "%02x".format(it) }

class ModelManagerTest {
    private val root: File = Files.createTempDirectory("models").toFile()
    private val big = ByteArray(200_000) { (it * 31 % 251).toByte() }
    private val small = "tokens".toByteArray()
    private val fetcher = FakeFetcher(mapOf("https://m/enc" to big, "https://m/tok" to small))
    private val settings = MemorySettings()

    private fun model(encSha: String = sha(big), id: String = "m") = AsrModel(
        id = id,
        name = "M",
        languages = listOf("en"),
        license = "test",
        files = listOf(
            ModelFile("enc.onnx", "https://m/enc", big.size.toLong(), encSha),
            ModelFile("tokens.txt", "https://m/tok", small.size.toLong(), sha(small)),
        ),
        layout = TransducerLayout("enc.onnx", "enc.onnx", "enc.onnx", "tokens.txt", "zipformer"),
        recommendedRamBytes = 0,
    )

    private fun manager(vararg models: AsrModel) = ModelManager(root, settings, fetcher, models.toList())

    @After
    fun cleanup() {
        root.deleteRecursively()
    }

    @Test
    fun downloadChecksEachFileAndInstalls() {
        val m = model()
        val mm = manager(m)
        assertEquals(ModelState.NotInstalled, mm.state(m))
        var last = 0L to 0L
        mm.download(m, onProgress = { d, t -> last = d to t })
        assertTrue(mm.isInstalled(m))
        assertEquals(ModelState.Installed, mm.state(m))
        assertEquals(m.downloadBytes to m.downloadBytes, last)
        assertTrue(File(mm.modelDir(m), "enc.onnx").readBytes().contentEquals(big))
        assertFalse(File(mm.modelDir(m), "enc.onnx.part").exists())
    }

    @Test
    fun aFileWithTheWrongHashIsDeletedAndTheModelIsNotInstalled() {
        val m = model(encSha = "0".repeat(64))
        val mm = manager(m)
        try {
            mm.download(m)
            fail("expected a failed check")
        } catch (e: IOException) {
            assertTrue(e.message!!.contains("failed its check"))
        }
        assertFalse(mm.isInstalled(m))
        assertFalse(File(mm.modelDir(m), "enc.onnx.part").exists())
        assertFalse(File(mm.modelDir(m), "enc.onnx").exists())
    }

    @Test
    fun anInterruptedDownloadResumesFromThePartialFile() {
        val m = model()
        val mm = manager(m)
        fetcher.failAfter = 70_000
        try {
            mm.download(m)
            fail("expected the connection to drop")
        } catch (e: IOException) {
            assertEquals("connection reset", e.message)
        }
        assertEquals(ModelState.Partial(70_000, m.downloadBytes), mm.state(m))
        mm.download(m)
        assertEquals(listOf(0L, 70_000L, 0L), fetcher.offsets)
        assertTrue(mm.isInstalled(m))
        assertTrue(File(mm.modelDir(m), "enc.onnx").readBytes().contentEquals(big))
    }

    @Test
    fun aServerThatIgnoresRangeRestartsTheFileInsteadOfAppending() {
        val m = model()
        val mm = manager(m)
        fetcher.failAfter = 50_000
        runCatching { mm.download(m) }
        fetcher.ignoreRange = true
        mm.download(m)
        assertTrue(mm.isInstalled(m))
        assertTrue(File(mm.modelDir(m), "enc.onnx").readBytes().contentEquals(big))
    }

    @Test
    fun cancelKeepsThePartialFileForLater() {
        val m = model()
        val mm = manager(m)
        var reads = 0
        try {
            mm.download(m, isCancelled = { ++reads > 2 })
            fail("expected cancel")
        } catch (_: ModelDownloadCancelled) {
        }
        assertTrue(mm.state(m) is ModelState.Partial)
        assertFalse(mm.isInstalled(m))
    }

    @Test
    fun aTruncatedFileIsNotInstalled() {
        val m = model()
        val mm = manager(m)
        mm.download(m)
        File(mm.modelDir(m), "enc.onnx").writeBytes(big.copyOf(10))
        assertFalse(mm.isInstalled(m))
        mm.download(m)
        assertTrue(mm.isInstalled(m))
    }

    @Test
    fun setActiveNeedsAnInstalledModelAndReplacesTheActiveOne() {
        val first = model(id = "first")
        val second = model(id = "second")
        val mm = manager(first, second)
        assertFalse(mm.setActive(first))
        assertNull(settings.value.modelId)
        mm.download(first)
        mm.download(second)
        assertTrue(mm.setActive(first))
        assertEquals(first, mm.activeModel())
        assertTrue(mm.setActive(second))
        assertEquals("second", settings.value.modelId)
        assertEquals(second, mm.activeModel())
    }

    @Test
    fun anActiveIdOutsideTheCatalogIsNotActive() {
        val m = model()
        val mm = manager(m)
        mm.download(m)
        settings.value = VoiceSettings(modelId = "zipformer-en-streaming-2023-06-26")
        assertNull(mm.activeModel())
    }

    @Test
    fun deleteRetiredKeepsCatalogModelsAndRemovesTheRest() {
        val m = model()
        val mm = manager(m)
        mm.download(m)
        val retired = File(root, "zipformer-en-streaming-2023-06-26").apply { mkdirs() }
        File(retired, "encoder.onnx").writeBytes(small)
        val stray = File(root, "notes.txt").apply { writeText("x") }
        mm.deleteRetired()
        assertFalse(retired.exists())
        assertTrue(stray.exists())
        assertTrue(mm.isInstalled(m))
    }

    @Test
    fun deleteRemovesTheFilesAndTheActiveSetting() {
        val m = model()
        val mm = manager(m)
        mm.download(m)
        mm.setActive(m)
        mm.delete(m)
        assertFalse(mm.modelDir(m).exists())
        assertNull(settings.value.modelId)
        assertNull(mm.activeModel())
    }

    @Test
    fun anActiveModelWhoseFilesAreGoneIsNotActive() {
        val m = model()
        val mm = manager(m)
        mm.download(m)
        mm.setActive(m)
        File(mm.modelDir(m), "tokens.txt").delete()
        assertNull(mm.activeModel())
    }

    @Test
    fun ramAdvisoryAllowsWhatA6GbPhoneReportsAndWarnsBelow() {
        val parakeet = ModelCatalog.PARAKEET_V3
        val mm = manager(parakeet)
        val gb = 1L shl 30
        assertNull(mm.ramAdvisory(parakeet, 5_500_000_000)) // a 6 GB phone's totalMem
        assertNull(mm.ramAdvisory(parakeet, 8 * gb))
        assertEquals(RamAdvisory(parakeet, 4 * gb), mm.ramAdvisory(parakeet, 4 * gb))
        assertNull(mm.ramAdvisory(model(), 2 * gb))
    }
}

class ModelCatalogTest {
    @Test
    fun everyFileIsPinnedAndChecked() {
        val hex64 = Regex("[0-9a-f]{64}")
        val pinned = Regex("""^https://huggingface\.co/csukuangfj/[^/]+/resolve/[0-9a-f]{40}/[^/]+$""")
        assertEquals(ModelCatalog.ALL.size, ModelCatalog.ALL.map { it.id }.toSet().size)
        for (m in ModelCatalog.ALL) {
            for (f in m.files) {
                assertTrue("${m.id}/${f.name} sha256", hex64.matches(f.sha256))
                assertTrue("${m.id}/${f.name} url ${f.url}", pinned.matches(f.url))
                assertTrue("${m.id}/${f.name} url ends with its name", f.url.endsWith("/" + f.name))
                assertTrue(f.sizeBytes > 0)
            }
            val names = m.files.map { it.name }.toSet()
            with(m.layout) { listOf(encoder, decoder, joiner, tokens).forEach { assertTrue("${m.id} lacks $it", it in names) } }
        }
    }

    @Test
    fun theCatalogHasParakeetAndNoStreamingZipformer() {
        assertEquals(6L shl 30, ModelCatalog.PARAKEET_V3.recommendedRamBytes)
        assertTrue(ModelCatalog.PARAKEET_V3 in ModelCatalog.ALL)
        assertNull(ModelCatalog.byId("zipformer-en-streaming-2023-06-26"))
        assertTrue(ModelCatalog.ALL.none { it.id.contains("streaming") || it.layout.modelType == "zipformer2" })
    }
}
