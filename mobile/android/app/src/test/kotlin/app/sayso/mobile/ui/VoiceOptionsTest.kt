package app.sayso.mobile.ui

import app.sayso.mobile.asr.ModelCatalog
import app.sayso.mobile.asr.ModelState
import app.sayso.mobile.asr.RamAdvisory
import app.sayso.mobile.engine.CloudAsrProvider
import app.sayso.mobile.engine.CloudStyleProvider
import app.sayso.mobile.engine.VoiceStyle
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class VoiceOptionsTest {
    private val running = DownloadStatus.Running(10, 100)
    private val failed = DownloadStatus.Failed("HTTP 500")
    private val partial = ModelState.Partial(10, 100)

    @Test
    fun badgePrefersARunningDownload() {
        assertEquals(ModelBadge.DOWNLOADING, modelBadge(partial, running, active = false))
        assertEquals(ModelBadge.DOWNLOADING, modelBadge(ModelState.NotInstalled, running, active = false))
    }

    @Test
    fun badgeFromDisk() {
        assertEquals(ModelBadge.NOT_DOWNLOADED, modelBadge(ModelState.NotInstalled, null, active = false))
        assertEquals(ModelBadge.PAUSED, modelBadge(partial, null, active = false))
        assertEquals(ModelBadge.DOWNLOADED, modelBadge(ModelState.Installed, null, active = false))
        assertEquals(ModelBadge.ACTIVE, modelBadge(ModelState.Installed, null, active = true))
    }

    @Test
    fun failureShowsUntilInstalled() {
        assertEquals(ModelBadge.FAILED, modelBadge(partial, failed, active = false))
        assertEquals(ModelBadge.FAILED, modelBadge(ModelState.NotInstalled, failed, active = false))
        assertEquals(ModelBadge.DOWNLOADED, modelBadge(ModelState.Installed, failed, active = false))
    }

    @Test
    fun percent() {
        assertEquals(0, downloadPercent(0, 0))
        assertEquals(0, downloadPercent(5, 1000))
        assertEquals(42, downloadPercent(42, 100))
        assertEquals(99, downloadPercent(999_999, 1_000_000))
        assertEquals(100, downloadPercent(100, 100))
    }

    @Test
    fun sizesMatchTheReadme() {
        assertEquals("670.6 MB", formatBytes(ModelCatalog.PARAKEET_V3.downloadBytes))
        assertEquals("1.50 GB", formatBytes(1_500_000_000))
        assertEquals("6 KB", formatBytes(5_048))
        assertEquals("6.0 GB", formatRam(6L shl 30))
    }

    @Test
    fun ramNoteOnlyForModelsThatNeedMemory() {
        assertNull(ramNote(ModelCatalog.MOONSHINE_TINY, null))
        assertEquals(RamNote("Wants 6.0 GB of memory.", warning = false), ramNote(ModelCatalog.PARAKEET_ULTRA, null))
        val low = ramNote(ModelCatalog.PARAKEET_ULTRA, RamAdvisory(ModelCatalog.PARAKEET_ULTRA, 4L shl 30))!!
        assertTrue(low.warning)
        assertEquals("This phone has 4.0 GB of memory; this model wants 6.0 GB. It may be slow or stop.", low.text)
    }

    @Test
    fun modelSizesAtAGlance() {
        assertEquals(listOf("124 MB", "161 MB", "376 MB", "671 MB", "671 MB"), ModelCatalog.ALL.map { formatSize(it.downloadBytes) })
        assertEquals("1.5 GB", formatSize(1_500_000_000))
    }

    @Test
    fun modelLinesReadAsTierAndLanguages() {
        assertEquals(
            listOf("Fastest, compact · English", "Fast Whisper · English", "Accurate Whisper · English", "Best accuracy · 25 languages", "Multilingual · 25 languages"),
            ModelCatalog.ALL.map(::modelLine),
        )
    }

    @Test
    fun languages() {
        assertEquals("25 languages", modelLanguages(ModelCatalog.PARAKEET_V3))
        assertEquals("English", modelLanguages(ModelCatalog.PARAKEET_V3.copy(languages = listOf("en"))))
    }

    @Test
    fun unknownStyleFallsBackToTheFirst() {
        val styles = listOf(VoiceStyle("clean", "Clean", "", true), VoiceStyle("email", "Email", "p", true))
        assertEquals("email", effectiveStyleId(styles, "email"))
        assertEquals("clean", effectiveStyleId(styles, "gone"))
        assertNull(effectiveStyleId(emptyList(), "clean"))
    }

    @Test
    fun compatibleServersNeedAUrlAndModel() {
        assertTrue(asrHints(CloudAsrProvider.OPENAI_COMPATIBLE).needsBaseUrl)
        assertTrue(styleHints(CloudStyleProvider.OPENAI_COMPATIBLE).needsModel)
        assertFalse(asrHints(CloudAsrProvider.GROQ).needsBaseUrl)
        assertEquals("whisper-large-v3-turbo", asrHints(CloudAsrProvider.GROQ).model)
        assertEquals("claude-haiku-4-5", styleHints(CloudStyleProvider.ANTHROPIC).model)
    }
}
