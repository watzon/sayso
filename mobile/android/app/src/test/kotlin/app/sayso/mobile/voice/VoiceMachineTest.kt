package app.sayso.mobile.voice

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class VoiceMachineTest {
    private val m = VoiceMachine()

    @Test
    fun aDictationListensStopsTranscribesAndCompletes() {
        val s = m.start(VoiceEngine.LOCAL, 1_000)!!
        assertTrue(m.partial(s, "hello"))
        assertEquals(VoiceState.Listening(s, VoiceEngine.LOCAL, 1_000, "hello"), m.state)
        assertTrue(m.stop(s, 4_000))
        assertEquals(VoiceState.Finishing(s, VoiceEngine.LOCAL, 1_000, 4_000, "hello"), m.state)
        assertFalse("no partials after stop", m.partial(s, "hello wor"))
        assertTrue(m.transcribed(s, 4_200))
        assertTrue((m.state as VoiceState.Finishing).styling)
        assertTrue(m.complete(s))
        assertEquals(VoiceState.Idle, m.state)
    }

    @Test
    fun nothingFromACancelledDictationIsInserted() {
        val s = m.start(VoiceEngine.LOCAL, 0)!!
        m.stop(s, 10)
        m.cancel()
        assertFalse(m.transcribed(s, 20))
        assertFalse(m.complete(s))
        assertEquals(VoiceState.Idle, m.state)
    }

    @Test
    fun cancelWhileStylingDropsTheStyledText() {
        val s = m.start(VoiceEngine.CLOUD, 0)!!
        m.stop(s, 10)
        m.transcribed(s, 20)
        m.cancel()
        assertFalse(m.complete(s))
    }

    @Test
    fun resultsOfAnOlderSessionAreIgnoredByTheNewOne() {
        val old = m.start(VoiceEngine.LOCAL, 0)!!
        m.cancel()
        val now = m.start(VoiceEngine.LOCAL, 100)!!
        assertNotEquals(old, now)
        assertFalse(m.partial(old, "late"))
        assertFalse(m.transcribed(old, 200))
        assertFalse(m.fail(old, "late error"))
        assertEquals(VoiceState.Listening(now, VoiceEngine.LOCAL, 100), m.state)
    }

    @Test
    fun onlyOneDictationAtATime() {
        val s = m.start(VoiceEngine.SYSTEM, 0)!!
        assertNull(m.start(VoiceEngine.SYSTEM, 1))
        m.stop(s, 2)
        assertNull(m.start(VoiceEngine.SYSTEM, 3))
        m.fail(s, "boom")
        assertTrue("a failed dictation can be retried", m.start(VoiceEngine.SYSTEM, 4) != null)
    }

    @Test
    fun theSystemRecognizerMayEndByItselfWhileListening() {
        val s = m.start(VoiceEngine.SYSTEM, 0)!!
        assertTrue(m.transcribed(s, 3_000))
        assertTrue(m.complete(s))
    }

    @Test
    fun aFinalArrivesOnce() {
        val s = m.start(VoiceEngine.LOCAL, 0)!!
        m.stop(s, 1)
        assertTrue(m.transcribed(s, 2))
        assertFalse(m.transcribed(s, 3))
        assertTrue(m.complete(s))
        assertFalse(m.complete(s))
    }

    @Test
    fun switchingEngineStartsANewSessionWithANotice() {
        val s = m.start(VoiceEngine.LOCAL, 0)!!
        val next = m.switchEngine(s, VoiceEngine.SYSTEM, 500, "switched")!!
        assertEquals(VoiceState.Listening(next, VoiceEngine.SYSTEM, 500, notice = "switched"), m.state)
        assertFalse("the dead engine's final is dropped", m.transcribed(s, 600))
        assertNull("only the current session can switch", m.switchEngine(s, VoiceEngine.SYSTEM, 700, "again"))
    }

    @Test
    fun failureShowsAMessageAndNothingCompletes() {
        val s = m.start(VoiceEngine.LOCAL, 0)!!
        assertTrue(m.fail(s, "Didn't catch that"))
        assertEquals(VoiceState.Failed("Didn't catch that"), m.state)
        assertFalse(m.complete(s))
        m.cancel()
        assertEquals(VoiceState.Idle, m.state)
    }

    @Test
    fun longRecordingsStopByThemselves() {
        m.start(VoiceEngine.LOCAL, 1_000)
        assertFalse(m.shouldAutoStop(1_000 + VoiceMachine.MAX_RECORDING_MS - 1))
        assertTrue(m.shouldAutoStop(1_000 + VoiceMachine.MAX_RECORDING_MS))
    }
}
