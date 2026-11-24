package app.sayso.mobile.keyboard

import app.sayso.mobile.engine.CommitAction
import app.sayso.mobile.settings.KeyboardSettings
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class DoubleSpacePeriodTest {
    /** The editor text, driven the way SaysoImeService drives it. */
    private class Field(text: String = "", val allows: Boolean = true) {
        val buffer = StringBuilder(text)
        private val editor = object : TextEditor {
            override fun textBeforeCursor(length: Int) = buffer.substring(maxOf(0, buffer.length - length))
            override fun deleteBefore(length: Int) {
                buffer.setLength(buffer.length - length)
            }
            override fun insert(text: String) {
                buffer.append(text)
            }
            override fun batch(block: () -> Unit) = block()
        }
        private val session = EditorSession(editor, buffer.length)
        val period = DoubleSpacePeriod()

        fun space(atMs: Long) {
            session.apply(period.onSpace(editor.textBeforeCursor(2), atMs, allows) ?: CommitAction(0, " "))
        }

        fun type(text: String) {
            period.reset()
            session.insert(text)
        }

        fun backspace() {
            val undo = period.onBackspace(editor.textBeforeCursor(2))
            if (undo != null) session.apply(undo) else session.deleteCharBefore()
        }

        override fun toString() = buffer.toString()
    }

    @Test
    fun itIsOnByDefault() {
        assertTrue(KeyboardSettings().doubleSpacePeriod)
        assertTrue(DoubleSpacePeriod().enabled)
    }

    @Test
    fun twoQuickSpacesAfterAWordTypeAPeriod() {
        val f = Field("Hello")
        f.space(1_000)
        f.space(1_300)
        assertEquals("Hello. ", f.toString())
        f.type("World")
        assertEquals("Hello. World", f.toString())
    }

    @Test
    fun theWindowIsInclusiveAndASlowSecondSpaceIsJustASpace() {
        val quick = Field("ok")
        quick.space(0)
        quick.space(DoubleSpacePeriod.WINDOW_MS)
        assertEquals("ok. ", quick.toString())

        val slow = Field("ok")
        slow.space(0)
        slow.space(DoubleSpacePeriod.WINDOW_MS + 1)
        assertEquals("ok  ", slow.toString())
    }

    @Test
    fun turnedOffTwoSpacesStayTwoSpaces() {
        val f = Field("Hello")
        f.period.enabled = false
        f.space(1_000)
        f.space(1_100)
        assertEquals("Hello  ", f.toString())
    }

    @Test
    fun aDigitCountsAsTheEndOfAWord() {
        val f = Field("at 5")
        f.space(0)
        f.space(100)
        assertEquals("at 5. ", f.toString())
    }

    @Test
    fun notAtTheStartOrAfterWhitespaceOrPunctuation() {
        for (start in listOf("", " ", "Hi ", "line\n", "Done.", "Wait,", "Yes!", "Why?", "a:", "a;", "a-", "\"quote\"", "(x)", "it's'")) {
            val f = Field(start)
            f.space(0)
            f.space(100)
            assertEquals("after \"$start\"", "$start  ", f.toString())
        }
    }

    @Test
    fun notInFieldsThatDoNotAllowIt() {
        val f = Field("user", allows = false)
        f.space(0)
        f.space(100)
        assertEquals("user  ", f.toString())
    }

    @Test
    fun aKeyBetweenTheSpacesResetsTheTimer() {
        val f = Field("Hello")
        f.space(0)
        f.type("a")
        f.backspace()
        f.space(200)
        assertEquals("Hello  ", f.toString())
    }

    @Test
    fun aThirdQuickSpaceIsAPlainSpace() {
        val f = Field("Hello")
        f.space(0)
        f.space(100)
        f.space(200)
        assertEquals("Hello.  ", f.toString())
    }

    @Test
    fun backspaceRightAfterThePeriodPutsTheSpaceBack() {
        val f = Field("Hello")
        f.space(0)
        f.space(100)
        f.backspace()
        assertEquals("Hello ", f.toString())
        f.backspace()
        assertEquals("Hello", f.toString())
    }

    @Test
    fun backspaceAfterTypingOnDeletesNormally() {
        val f = Field("Hello")
        f.space(0)
        f.space(100)
        f.type("W")
        f.backspace()
        assertEquals("Hello. ", f.toString())
        f.backspace()
        assertEquals("Hello.", f.toString())
    }

    @Test
    fun theUndoNeedsThePeriodStillBeforeTheCursor() {
        val p = DoubleSpacePeriod()
        p.onSpace("Hello", 0, true)
        assertEquals(CommitAction(1, ". "), p.onSpace("Hello ", 100, true))
        assertNull("the text changed under us", p.onBackspace("o "))
    }

    @Test
    fun followsAWord() {
        assertTrue(DoubleSpacePeriod.followsAWord("word "))
        assertTrue(DoubleSpacePeriod.followsAWord("é "))
        assertFalse(DoubleSpacePeriod.followsAWord("word"))
        assertFalse(DoubleSpacePeriod.followsAWord("word  "))
        assertFalse(DoubleSpacePeriod.followsAWord(". "))
        assertFalse(DoubleSpacePeriod.followsAWord(" "))
    }
}
