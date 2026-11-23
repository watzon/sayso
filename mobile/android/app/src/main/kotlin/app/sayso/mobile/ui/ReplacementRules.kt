package app.sayso.mobile.ui

import app.sayso.mobile.engine.Replacement
import app.sayso.mobile.engine.isWordChar

/**
 * The limits the typing core keeps a replacement within, in UTF-8 bytes
 * (mobile/core/learner: maxWordLen and maxReplacementLen). The core refuses
 * anything longer.
 */
const val MAX_SHORTCUT_BYTES = 128
const val MAX_REPLACEMENT_TEXT_BYTES = 512

/** Why a field cannot be saved, or null when it can. Blank fields are not errors: the Add button is off. */
data class ReplacementErrors(val shortcut: String?, val text: String?) {
    val any: Boolean get() = shortcut != null || text != null
}

/** Checks a replacement the way the engines store it: trimmed, the shortcut lower-cased. */
fun validateReplacement(shortcut: String, text: String): ReplacementErrors {
    val s = shortcut.trim().lowercase()
    val t = text.trim()
    val shortcutError = when {
        s.any { !isWordChar(it) } -> "Use letters, digits and apostrophes only, with no spaces."
        else -> tooLong(s, MAX_SHORTCUT_BYTES)
    }
    return ReplacementErrors(shortcutError, tooLong(t, MAX_REPLACEMENT_TEXT_BYTES))
}

/** True when [saved] holds [shortcut] with [text], as the engine normalizes them, so the add was kept. */
fun replacementKept(saved: List<Replacement>, shortcut: String, text: String): Boolean {
    val s = shortcut.trim().lowercase()
    val t = text.trim()
    return saved.any { it.shortcut.equals(s, ignoreCase = true) && it.expansion == t }
}

private fun tooLong(value: String, max: Int): String? {
    val bytes = value.encodeToByteArray().size
    if (bytes <= max) return null
    return "Too long: $bytes of $max allowed. Accented letters count as 2, emoji as 4."
}
