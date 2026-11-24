package app.sayso.mobile.keyboard

import app.sayso.mobile.engine.CommitAction

/**
 * Two quick taps on space after a word type ". ". The second tap must come
 * within [windowMs] of the first, with no other key between them, and the
 * text before it must end in a letter or digit and one space. A backspace
 * right after it puts the single space back.
 *
 * The IME calls [onSpace] before the space goes to the engine, [onBackspace]
 * before a delete, and [reset] for every other key and for cursor moves.
 */
class DoubleSpacePeriod(private val windowMs: Long = WINDOW_MS) {
    var enabled = true
        set(value) {
            field = value
            reset()
        }

    private var lastSpaceMs = NO_SPACE
    private var justInserted = false

    /**
     * A tap on space. [textBefore] is the text before the cursor, without this
     * space. [fieldAllows] is false in fields where a period would be wrong
     * (passwords, email addresses, URLs, numbers). Returns the edit to apply
     * in place of the space, or null to type the space as usual.
     */
    fun onSpace(textBefore: String, nowMs: Long, fieldAllows: Boolean): CommitAction? {
        val quick = lastSpaceMs != NO_SPACE && nowMs - lastSpaceMs in 0..windowMs
        justInserted = false
        if (enabled && fieldAllows && quick && followsAWord(textBefore)) {
            lastSpaceMs = NO_SPACE
            justInserted = true
            return CommitAction(1, ". ")
        }
        lastSpaceMs = nowMs
        return null
    }

    /** A delete. Returns the edit that turns a just-typed ". " back into " ", or null. */
    fun onBackspace(textBefore: String): CommitAction? {
        val undo = justInserted && textBefore.endsWith(". ")
        reset()
        return if (undo) CommitAction(2, " ") else null
    }

    fun reset() {
        lastSpaceMs = NO_SPACE
        justInserted = false
    }

    companion object {
        const val WINDOW_MS = 450L
        private const val NO_SPACE = Long.MIN_VALUE

        /** "word " : one space after a letter or digit. */
        fun followsAWord(textBefore: String): Boolean =
            textBefore.length >= 2 && textBefore.last() == ' ' && textBefore[textBefore.length - 2].isLetterOrDigit()
    }
}
