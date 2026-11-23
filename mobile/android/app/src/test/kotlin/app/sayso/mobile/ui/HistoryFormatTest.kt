package app.sayso.mobile.ui

import org.junit.Assert.assertEquals
import org.junit.Test
import java.time.LocalDateTime
import java.time.ZoneId
import java.util.Locale

class HistoryFormatTest {
    private val zone = ZoneId.of("America/Denver")
    private val now = LocalDateTime.of(2026, 11, 23, 15, 30).atZone(zone).toInstant().toEpochMilli()

    private fun at(y: Int, mo: Int, d: Int, h: Int, mi: Int) =
        LocalDateTime.of(y, mo, d, h, mi).atZone(zone).toInstant().toEpochMilli()

    private fun fmt(ts: Long, is24: Boolean = false) = formatWhen(ts, now, zone, Locale.US, is24)

    @Test
    fun durations() {
        assertEquals("0:00", formatDuration(0))
        assertEquals("0:00", formatDuration(999))
        assertEquals("0:07", formatDuration(7_400))
        assertEquals("1:05", formatDuration(65_000))
        assertEquals("59:59", formatDuration(3_599_000))
        assertEquals("1:00:03", formatDuration(3_603_000))
        assertEquals("0:00", formatDuration(-5_000))
    }

    @Test
    fun recentTimesAreRelative() {
        assertEquals("Just now", fmt(now - 59_000))
        assertEquals("Just now", fmt(now + 120_000))
        assertEquals("1 min ago", fmt(now - 60_000))
        assertEquals("59 min ago", fmt(now - 3_599_000))
    }

    @Test
    fun olderTimesUseTheLocalClockAndDate() {
        assertEquals("Today, 9:05 AM", fmt(at(2026, 11, 23, 9, 5)))
        assertEquals("Today, 09:05", fmt(at(2026, 11, 23, 9, 5), is24 = true))
        assertEquals("Yesterday, 11:50 PM", fmt(at(2026, 11, 22, 23, 50)))
        assertEquals("Nov 3, 2:00 PM", fmt(at(2026, 11, 3, 14, 0)))
        assertEquals("Dec 31, 2025, 8:15 PM", fmt(at(2025, 12, 31, 20, 15)))
    }
}
