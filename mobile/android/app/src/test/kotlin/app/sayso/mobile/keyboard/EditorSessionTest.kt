package app.sayso.mobile.keyboard

import app.sayso.mobile.engine.CommitAction
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** An editor whose cursor is always at the end of [text]. */
private class FakeEditor(initial: String = "") : TextEditor {
    val text = StringBuilder(initial)
    var batches = 0

    override fun textBeforeCursor(length: Int) = text.substring(maxOf(0, text.length - length))
    override fun deleteBefore(length: Int) {
        text.setLength(text.length - length)
    }
    override fun insert(text: String) {
        this.text.append(text)
    }
    override fun batch(block: () -> Unit) {
        batches++
        block()
    }
}

class EditorSessionTest {
    @Test
    fun autocorrectReplacesTheWordAndAddsTheSeparatorInOneBatch() {
        val editor = FakeEditor("I said teh")
        val session = EditorSession(editor, 10)
        session.apply(CommitAction(3, "the ", wasAutocorrect = true, original = "teh"))
        assertEquals("I said the ", editor.text.toString())
        assertEquals(11, session.expectedCursor)
        assertEquals(1, editor.batches)
    }

    @Test
    fun deleteNeverReachesPastTheStartOfTheText() {
        val editor = FakeEditor("hi")
        val session = EditorSession(editor, 2)
        session.apply(CommitAction(5, "yo"))
        assertEquals("yo", editor.text.toString())
        assertEquals(2, session.expectedCursor)
    }

    @Test
    fun lateSelectionUpdatesFromOwnEditsAreNotUserMoves() {
        val editor = FakeEditor()
        val session = EditorSession(editor, 0)
        session.insert("t")
        session.insert("e")
        session.insert("h")
        session.apply(CommitAction(3, "the ", wasAutocorrect = true, original = "teh"))
        // The editor reports each position after the fact.
        assertFalse(session.movedExternally(1, 1))
        assertFalse(session.movedExternally(3, 3))
        assertFalse(session.movedExternally(4, 4))
        // A tap that moves the cursor, or a selection, is the user's.
        assertTrue(session.movedExternally(2, 2))
        assertTrue(session.movedExternally(4, 5))
    }

    @Test
    fun deleteCharRemovesWholeSurrogatePairs() {
        val editor = FakeEditor("ok 👍")
        val session = EditorSession(editor, 5)
        assertTrue(session.deleteCharBefore())
        assertEquals("ok ", editor.text.toString())
        assertEquals(3, session.expectedCursor)
    }

    @Test
    fun deleteCharOnEmptyTextReportsNothingDeleted() {
        assertFalse(EditorSession(FakeEditor(), 0).deleteCharBefore())
    }

    @Test
    fun wordDeleteTakesTrailingSpacesWithTheWord() {
        assertEquals(7, EditorSession.wordDeleteLength("hello world  "))
        assertEquals(5, EditorSession.wordDeleteLength("I don't"))
        assertEquals(3, EditorSession.wordDeleteLength("wow!!!"))
        assertEquals(0, EditorSession.wordDeleteLength(""))
    }
}
