package app.sayso.mobile.ui

import app.sayso.mobile.engine.Replacement
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ReplacementRulesTest {
    @Test
    fun validReplacementHasNoErrors() {
        val e = validateReplacement("  OMW ", "On my way!")
        assertFalse(e.any)
    }

    @Test
    fun blankFieldsAreNotErrors() {
        assertFalse(validateReplacement("", "").any)
    }

    @Test
    fun shortcutMustBeOneWord() {
        assertNotNull(validateReplacement("on my", "On my way").shortcut)
        assertNotNull(validateReplacement("a-b", "x").shortcut)
        assertNull(validateReplacement("don't", "do not").shortcut)
    }

    @Test
    fun limitsAreUtf8BytesAtTheCoreMaximum() {
        assertNull(validateReplacement("a".repeat(MAX_SHORTCUT_BYTES), "x").shortcut)
        assertEquals(
            "Too long: 129 of 128 allowed. Accented letters count as 2, emoji as 4.",
            validateReplacement("a".repeat(MAX_SHORTCUT_BYTES + 1), "x").shortcut,
        )
        // 65 two-byte letters are 130 bytes, over the limit though only 65 characters.
        assertNotNull(validateReplacement("é".repeat(65), "x").shortcut)

        assertNull(validateReplacement("x", "a".repeat(MAX_REPLACEMENT_TEXT_BYTES)).text)
        assertNotNull(validateReplacement("x", "a".repeat(MAX_REPLACEMENT_TEXT_BYTES + 1)).text)
        assertNotNull(validateReplacement("x", "🙂".repeat(129)).text)
        // Surrounding spaces are trimmed before the engine sees the text.
        assertNull(validateReplacement("x", " " + "a".repeat(MAX_REPLACEMENT_TEXT_BYTES) + " ").text)
    }

    @Test
    fun keptMatchesTheEngineNormalization() {
        val saved = listOf(Replacement("omw", "On my way!"))
        assertTrue(replacementKept(saved, " OMW ", "On my way! "))
        assertFalse(replacementKept(saved, "omw", "On my way"))
        assertFalse(replacementKept(emptyList(), "omw", "On my way!"))
    }
}
