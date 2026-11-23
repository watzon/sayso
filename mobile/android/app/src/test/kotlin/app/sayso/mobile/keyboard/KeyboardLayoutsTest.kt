package app.sayso.mobile.keyboard

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class KeyboardLayoutsTest {
    private val board = KeyboardLayouts.build(KeyboardMode.LETTERS, width = 1000f, height = 400f, gapX = 10f, gapY = 20f)

    private fun key(label: String) = board.keys.first { it.spec.label == label }

    @Test
    fun lettersHaveFourRowsAndAllTwentySixLetters() {
        val letters = board.keys.filter { it.spec.kind == KeyKind.LETTER }.map { it.spec.label }
        assertEquals(('a'..'z').map(Char::toString).sorted(), letters.sorted())
        assertEquals(4, board.keys.map { it.top }.distinct().size)
    }

    @Test
    fun rowsScaleToTheWidestRowAndShortRowsAreCentered() {
        // Ten letters fill the top row: each cell is 100 px, keys are inset by half the gap.
        assertEquals(5f, key("q").left, 0.01f)
        assertEquals(995f, key("p").right, 0.01f)
        // Nine letters: half a cell of margin on each side.
        assertEquals(55f, key("a").left, 0.01f)
        assertEquals(945f, key("l").right, 0.01f)
        // Space is four units wide.
        assertEquals(390f, key("space").width, 0.01f)
    }

    @Test
    fun keyAtFindsTheKeyInTheGapAndTheNearestOutsideAnyCell() {
        // Between q and w, inside q's half of the gap.
        assertEquals("q", board.keyAt(98f, 50f)?.spec?.label)
        assertEquals("w", board.keyAt(104f, 50f)?.spec?.label)
        // The margin left of "a" belongs to no cell; the nearest key is "a".
        assertEquals("a", board.keyAt(10f, 150f)?.spec?.label)
    }

    @Test
    fun engineLayoutHasOnlyLetters() {
        val layout = board.engineLayout()
        assertEquals(26, layout.keys.size)
        assertEquals(1000f, layout.width)
        assertTrue(layout.keys.all { it.label.single().isLetter() })
    }

    @Test
    fun everyModeBuildsWithinBounds() {
        for (mode in KeyboardMode.entries) {
            val b = KeyboardLayouts.build(mode, 720f, 300f, 8f, 12f)
            assertTrue(mode.name, b.keys.all { it.left >= 0f && it.right <= 720f && it.top >= 0f && it.bottom <= 300f })
            assertTrue(mode.name, b.keys.any { it.spec.kind == KeyKind.BACKSPACE } && b.keys.any { it.spec.kind == KeyKind.ENTER })
        }
    }

    @Test
    fun vowelsHaveAccentsOnLongPress() {
        assertTrue("é" in key("e").spec.longPress)
        assertTrue(key("q").spec.longPress.isEmpty())
    }
}
