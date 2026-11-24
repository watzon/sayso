package app.sayso.mobile.asr

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import kotlin.math.PI
import kotlin.math.sin

class SilenceTest {
    /** A 200 Hz tone with [rms] RMS for [ms] milliseconds at 16 kHz. */
    private fun tone(rms: Double, ms: Int) =
        FloatArray(16 * ms) { (rms * sqrt2 * sin(2 * PI * 200 * it / 16_000.0)).toFloat() }

    private val sqrt2 = kotlin.math.sqrt(2.0)

    @Test
    fun digitalSilenceIsSilence() {
        assertTrue(isSilence(FloatArray(16_000 * 4)))
        assertTrue(isSilence(FloatArray(0)))
    }

    @Test
    fun aQuietRoomFloorIsSilence() {
        val rng = java.util.Random(7)
        assertTrue(isSilence(FloatArray(16_000 * 4) { (rng.nextGaussian() * 0.0008).toFloat() }))
    }

    @Test
    fun quietSpeechIsNotSilence() {
        // -46 dBFS, far quieter than normal dictation, for a single word's length.
        val samples = FloatArray(16_000 * 4)
        tone(0.005, 200).copyInto(samples, 16_000 * 2)
        assertFalse(isSilence(samples))
    }

    @Test
    fun aShortFrameAtTheEndCounts() {
        val samples = FloatArray(1000)
        tone(0.01, 2).copyInto(samples, 1000 - 32)
        assertFalse(isSilence(samples))
    }

    @Test
    fun whisperTagsAloneBecomeEmpty() {
        for (t in listOf("[blank_audio]", "[BLANK_AUDIO]", " (buzzer)", "(buzzing)", "(silence)", "[Music]", "[ Silence ]", "(buzzing) [BLANK_AUDIO].")) {
            assertEquals(t, "", dropWhisperTags(t))
        }
    }

    @Test
    fun speechIsKeptAndBracketTagsAreCut() {
        assertEquals(
            "After early nightfall the yellow lamps would light up here and there.",
            dropWhisperTags(" After early nightfall the yellow lamps would light up here and there."),
        )
        assertEquals("Hello there.", dropWhisperTags("[BLANK_AUDIO] Hello there."))
        assertEquals("Call me (maybe) tomorrow.", dropWhisperTags("Call me (maybe) tomorrow."))
        assertEquals("you", dropWhisperTags("you"))
    }
}
