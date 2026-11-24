package app.sayso.mobile.ime

import app.sayso.mobile.voice.VoiceEngine
import app.sayso.mobile.voice.VoiceState
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class VoiceStripTest {
    private val density = 2.75f
    private val width = 1080f
    private val listening = VoiceState.Listening(session = 1, engine = VoiceEngine.LOCAL, startedAtMs = 10_000)
    private val finishing = VoiceState.Finishing(1, VoiceEngine.LOCAL, 10_000, 14_000, partial = "")
    private val failed = VoiceState.Failed("Didn't catch that")

    @Test
    fun cancelIsAtTheLeftAndStopAtTheRightWhileRecording() {
        assertEquals(VoiceStripAction.CANCEL, VoiceStrip.hit(10f, width, density, listening))
        assertEquals(VoiceStripAction.STOP, VoiceStrip.hit(width - 10f, width, density, listening))
        assertEquals(VoiceStripAction.NONE, VoiceStrip.hit(width / 2, width, density, listening))
    }

    @Test
    fun afterStopOnlyCancelWorksUntilTheTextArrives() {
        assertEquals(VoiceStripAction.NONE, VoiceStrip.primary(finishing))
        assertEquals(VoiceStripAction.NONE, VoiceStrip.hit(width - 10f, width, density, finishing))
        assertEquals(VoiceStripAction.CANCEL, VoiceStrip.hit(10f, width, density, finishing))
    }

    @Test
    fun anErrorOffersRetryAndClose() {
        assertEquals(VoiceStripAction.RETRY, VoiceStrip.hit(width - 10f, width, density, failed))
        assertEquals(VoiceStripAction.CANCEL, VoiceStrip.hit(10f, width, density, failed))
        assertEquals(VoiceStripStatus.Message("Didn't catch that", VoiceStripStatus.Tone.DANGER), VoiceStrip.status(failed, "", 0))
    }

    @Test
    fun idleTakesNoTaps() {
        assertEquals(VoiceStripAction.NONE, VoiceStrip.hit(10f, width, density, VoiceState.Idle))
        assertNull(VoiceStrip.status(VoiceState.Idle, "", 0))
    }

    @Test
    fun recordingShowsTheWaveAndTheTime() {
        assertEquals(VoiceStripStatus.Recording("0:07"), VoiceStrip.status(listening, "", 17_500))
        assertEquals(VoiceStripStatus.Recording("1:05"), VoiceStrip.status(listening, "", 75_000))
    }

    @Test
    fun finishingSaysTranscribingThenNamesAStyleOnlyWhenOneApplies() {
        assertEquals(VoiceStripStatus.Message("Transcribing…", VoiceStripStatus.Tone.QUIET), VoiceStrip.status(finishing, "Email", 0))
        val styling = finishing.copy(styling = true)
        assertEquals(VoiceStripStatus.Message("Applying Email…", VoiceStripStatus.Tone.QUIET), VoiceStrip.status(styling, "Email", 0))
        assertEquals(VoiceStripStatus.Message("Transcribing…", VoiceStripStatus.Tone.QUIET), VoiceStrip.status(styling, "", 0))
    }

    @Test
    fun aNoticeShowsBrieflyThenTheWaveReturns() {
        val switched = listening.copy(notice = "Using the system recognizer")
        assertEquals(
            VoiceStripStatus.Message("Using the system recognizer", VoiceStripStatus.Tone.QUIET),
            VoiceStrip.status(switched, "", 10_000 + VoiceStrip.NOTICE_MS - 1),
        )
        assertEquals(VoiceStripStatus.Recording("0:03"), VoiceStrip.status(switched, "", 10_000 + VoiceStrip.NOTICE_MS))
    }

    /** WCAG contrast of what the strip draws, in both system themes. */
    @Test
    fun stripColorsAreReadableInLightAndDark() {
        for (p in listOf(KeyboardPalette.LIGHT, KeyboardPalette.DARK)) {
            val name = if (p.dark) "dark" else "light"
            assertTrue("$name stop icon", contrast(p.onAccent, p.accent) >= 4.5)
            assertTrue("$name cancel icon", contrast(p.ink, p.modifierKey) >= 4.5)
            assertTrue("$name status text", contrast(p.graphite, p.ground) >= 4.5)
            assertTrue("$name error text", contrast(p.danger, p.ground) >= 4.5)
            assertTrue("$name waveform", contrast(p.accent, p.ground) >= 3.0)
        }
    }

    @Test
    fun thePaletteFollowsTheSystemNightMode() {
        // Configuration.UI_MODE_NIGHT_YES = 0x20, UI_MODE_NIGHT_NO = 0x10, UI_MODE_TYPE_NORMAL = 0x01.
        assertEquals(KeyboardPalette.DARK, KeyboardPalette.forUiMode(0x21))
        assertEquals(KeyboardPalette.LIGHT, KeyboardPalette.forUiMode(0x11))
        assertEquals(KeyboardPalette.LIGHT, KeyboardPalette.forUiMode(0))
    }

    private fun contrast(a: Int, b: Int): Double {
        val la = luminance(a)
        val lb = luminance(b)
        return (maxOf(la, lb) + 0.05) / (minOf(la, lb) + 0.05)
    }

    private fun luminance(argb: Int): Double {
        fun ch(shift: Int): Double {
            val c = ((argb shr shift) and 0xFF) / 255.0
            return if (c <= 0.03928) c / 12.92 else Math.pow((c + 0.055) / 1.055, 2.4)
        }
        return 0.2126 * ch(16) + 0.7152 * ch(8) + 0.0722 * ch(0)
    }
}
