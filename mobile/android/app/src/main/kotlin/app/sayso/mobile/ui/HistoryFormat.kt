package app.sayso.mobile.ui

import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.temporal.ChronoUnit
import java.util.Locale

/** A dictation length as m:ss, or h:mm:ss from an hour up. Negative lengths read 0:00. */
fun formatDuration(durationMs: Long): String {
    val total = durationMs.coerceAtLeast(0) / 1000
    val h = total / 3600
    val m = (total % 3600) / 60
    val s = total % 60
    return if (h > 0) "%d:%02d:%02d".format(h, m, s) else "%d:%02d".format(m, s)
}

/**
 * When a dictation happened, as the history list shows it: "Just now",
 * "12 min ago", then the local time for today, "Yesterday", the date this
 * year, or the full date. A timestamp ahead of [nowMs] (clock change) reads
 * "Just now".
 */
fun formatWhen(timestampMs: Long, nowMs: Long, zone: ZoneId, locale: Locale, is24Hour: Boolean): String {
    val ago = nowMs - timestampMs
    if (ago < 60_000) return "Just now"
    if (ago < 3_600_000) return "${ago / 60_000} min ago"
    val then = Instant.ofEpochMilli(timestampMs).atZone(zone)
    val now = Instant.ofEpochMilli(nowMs).atZone(zone)
    val time = then.format(DateTimeFormatter.ofPattern(if (is24Hour) "HH:mm" else "h:mm a", locale))
    val days = ChronoUnit.DAYS.between(then.toLocalDate(), now.toLocalDate())
    return when {
        days == 0L -> "Today, $time"
        days == 1L -> "Yesterday, $time"
        then.year == now.year -> "${then.format(DateTimeFormatter.ofPattern("MMM d", locale))}, $time"
        else -> "${then.format(DateTimeFormatter.ofPattern("MMM d, yyyy", locale))}, $time"
    }
}
