package app.sayso.mobile.keyboard

import app.sayso.mobile.engine.CommitAction
import app.sayso.mobile.engine.isWordChar

/** The few editor operations the keyboard uses. The IME backs it with an InputConnection. */
interface TextEditor {
    fun textBeforeCursor(length: Int): String
    fun deleteBefore(length: Int)
    fun insert(text: String)
    fun batch(block: () -> Unit)
}

/**
 * Applies edits to the editor and tracks where the cursor should be after
 * them, so the IME can tell its own cursor moves from moves the user or the
 * app made.
 */
class EditorSession(private val editor: TextEditor, cursor: Int) {
    var expectedCursor: Int = cursor
        private set

    // Cursor positions our edits produced that the editor has not reported yet.
    // Selection updates arrive late, so an update for an earlier edit is not a user move.
    private val pending = ArrayDeque<Int>()

    fun resetCursor(cursor: Int) {
        expectedCursor = cursor
        pending.clear()
    }

    /** True when the reported selection is not one our own edits produced. */
    fun movedExternally(selStart: Int, selEnd: Int): Boolean {
        if (selStart != selEnd) return true
        val index = pending.indexOf(selStart)
        if (index >= 0) {
            repeat(index + 1) { pending.removeFirst() }
            return false
        }
        if (selStart != expectedCursor) return true
        pending.clear()
        return false
    }

    private fun moveCursor(delta: Int) {
        expectedCursor += delta
        pending.addLast(expectedCursor)
        if (pending.size > MAX_PENDING) pending.removeFirst()
    }

    fun apply(action: CommitAction) {
        editor.batch {
            if (action.deleteBefore > 0) {
                val available = editor.textBeforeCursor(action.deleteBefore).length
                val length = minOf(action.deleteBefore, available)
                editor.deleteBefore(length)
                moveCursor(-length)
            }
            if (action.text.isNotEmpty()) insert(action.text)
        }
    }

    fun insert(text: String) {
        editor.insert(text)
        moveCursor(text.length)
    }

    /** Deletes one character, or a surrogate pair. Returns false when there was nothing to delete. */
    fun deleteCharBefore(): Boolean {
        val before = editor.textBeforeCursor(2)
        if (before.isEmpty()) return false
        val length = if (before.length == 2 && Character.isSurrogatePair(before[0], before[1])) 2 else 1
        editor.deleteBefore(length)
        moveCursor(-length)
        return true
    }

    /** Deletes the word before the cursor and the spaces after it. Returns false when there was nothing to delete. */
    fun deleteWordBefore(): Boolean {
        val length = wordDeleteLength(editor.textBeforeCursor(WORD_LOOKBACK))
        if (length == 0) return false
        editor.deleteBefore(length)
        moveCursor(-length)
        return true
    }

    companion object {
        private const val WORD_LOOKBACK = 64
        private const val MAX_PENDING = 64

        /** Trailing whitespace, then either a word or a run of other symbols. */
        fun wordDeleteLength(textBefore: String): Int {
            var i = textBefore.length
            while (i > 0 && textBefore[i - 1].isWhitespace()) i--
            if (i > 0 && isWordChar(textBefore[i - 1])) {
                while (i > 0 && isWordChar(textBefore[i - 1])) i--
            } else {
                while (i > 0 && !textBefore[i - 1].isWhitespace() && !isWordChar(textBefore[i - 1])) i--
            }
            return textBefore.length - i
        }
    }
}
