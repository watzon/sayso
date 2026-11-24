package app.sayso.mobile.asr

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class SegmentsTest {
    private fun loud(n: Int) = FloatArray(n) { if (it % 2 == 0) 0.5f else -0.5f }

    @Test
    fun aShortRecordingIsOnePiece() {
        assertEquals(listOf(0 until 100), splitAtPauses(loud(100), maxLen = 100, search = 50, frame = 10))
        assertEquals(listOf(0 until 0), splitAtPauses(FloatArray(0), maxLen = 100, search = 50, frame = 10))
    }

    @Test
    fun theCutFallsInThePauseBeforeTheLimit() {
        val samples = loud(250)
        for (i in 70 until 80) samples[i] = 0f
        val pieces = splitAtPauses(samples, maxLen = 100, search = 50, frame = 10)
        assertEquals(75, pieces[0].last + 1)
        assertEquals(75, pieces[1].first)
    }

    @Test
    fun piecesCoverEverySampleOnceAndStayWithinTheLimit() {
        val samples = FloatArray(10_007) { kotlin.math.sin(it * 0.37).toFloat() * (it % 997) / 997f }
        val pieces = splitAtPauses(samples, maxLen = 1_000, search = 300, frame = 30)
        assertEquals(0, pieces.first().first)
        assertEquals(samples.size - 1, pieces.last().last)
        pieces.zipWithNext { a, b -> assertEquals(a.last + 1, b.first) }
        assertTrue(pieces.all { it.count() in 1..1_000 })
    }

    @Test
    fun aOneSampleFrameStillMovesForward() {
        val pieces = splitAtPauses(loud(10), maxLen = 3, search = 3, frame = 1)
        assertEquals(10, pieces.sumOf { it.count() })
        assertTrue(pieces.all { it.count() in 1..3 })
    }
}
