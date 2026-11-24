package app.sayso.mobile.asr

import kotlin.math.sqrt

/**
 * True when no [frame]-sample window of [samples] reaches [threshold] RMS.
 * Whisper writes words like "you" or "(buzzing)" for silence, so silence is
 * never decoded. The default, about -54 dBFS, is near a quiet room's floor and
 * far below the loudest frame of even quiet speech.
 */
fun isSilence(samples: FloatArray, frame: Int = 480, threshold: Float = 0.002f): Boolean {
    require(frame > 0)
    var start = 0
    while (start < samples.size) {
        val end = minOf(samples.size, start + frame)
        var energy = 0.0
        for (i in start until end) energy += samples[i] * samples[i]
        if (sqrt(energy / (end - start)) >= threshold) return false
        start = end
    }
    return true
}

private val bracketTag = Regex("""\[[^\[\]]*]""")
private val parenTag = Regex("""\([^()]*\)""")
private val leftover = Regex("""[\s\p{Punct}]*""")

/**
 * [text] from Whisper without its non-speech tags. Tags in square brackets,
 * such as "[BLANK_AUDIO]" or "[Music]", are always dropped. Text made only of
 * tags, such as "(buzzing)", becomes empty; a parenthesis inside real words is kept.
 */
fun dropWhisperTags(text: String): String {
    val noBrackets = bracketTag.replace(text, " ")
    if (leftover.matches(parenTag.replace(noBrackets, " "))) return ""
    return noBrackets.replace(Regex("""\s+"""), " ").trim()
}
