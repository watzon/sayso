package app.sayso.mobile.keyboard

import android.text.InputType
import android.view.inputmethod.EditorInfo
import app.sayso.mobile.engine.InputKind
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class EditorFlagsMappingTest {
    private val text = InputType.TYPE_CLASS_TEXT

    @Test
    fun plainSentenceTextLearnsAndCapitalizes() {
        val flags = editorFlagsFor(text or InputType.TYPE_TEXT_FLAG_CAP_SENTENCES, 0)
        assertFalse(flags.privacy)
        assertTrue(flags.autoCapitalize)
        assertTrue(flags.suggestionsAllowed)
        assertEquals(InputKind.TEXT, flags.inputKind)
    }

    @Test
    fun everyPasswordVariationIsPrivateWithoutSuggestions() {
        val passwords = listOf(
            text or InputType.TYPE_TEXT_VARIATION_PASSWORD,
            text or InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD,
            text or InputType.TYPE_TEXT_VARIATION_WEB_PASSWORD,
            InputType.TYPE_CLASS_NUMBER or InputType.TYPE_NUMBER_VARIATION_PASSWORD,
        )
        for (type in passwords) {
            val flags = editorFlagsFor(type, 0)
            assertTrue("privacy for $type", flags.privacy)
            assertFalse("suggestions for $type", flags.suggestionsAllowed)
            assertEquals(InputKind.PASSWORD, flags.inputKind)
        }
    }

    @Test
    fun incognitoFieldsArePrivateButStillGetSuggestions() {
        val flags = editorFlagsFor(text, EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING)
        assertTrue(flags.privacy)
        assertTrue(flags.suggestionsAllowed)
    }

    @Test
    fun emailUrlAndNoSuggestionFieldsGetNoAutocorrect() {
        assertFalse(editorFlagsFor(text or InputType.TYPE_TEXT_VARIATION_EMAIL_ADDRESS, 0).suggestionsAllowed)
        assertFalse(editorFlagsFor(text or InputType.TYPE_TEXT_VARIATION_URI, 0).suggestionsAllowed)
        assertFalse(editorFlagsFor(text or InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS, 0).suggestionsAllowed)
        assertFalse(editorFlagsFor(text or InputType.TYPE_TEXT_VARIATION_EMAIL_ADDRESS or InputType.TYPE_TEXT_FLAG_CAP_SENTENCES, 0).autoCapitalize)
    }

    @Test
    fun numberAndPhoneFieldsMapToTheirKinds() {
        assertEquals(InputKind.NUMBER, editorFlagsFor(InputType.TYPE_CLASS_NUMBER, 0).inputKind)
        assertEquals(InputKind.PHONE, editorFlagsFor(InputType.TYPE_CLASS_PHONE, 0).inputKind)
    }

    @Test
    fun imeActionsMapToTheEnterKey() {
        assertEquals(ImeAction.SEARCH, imeActionFor(EditorInfo.IME_ACTION_SEARCH))
        assertEquals(ImeAction.SEND, imeActionFor(EditorInfo.IME_ACTION_SEND))
        assertEquals(ImeAction.GO, imeActionFor(EditorInfo.IME_ACTION_GO))
        assertEquals(ImeAction.DONE, imeActionFor(EditorInfo.IME_ACTION_DONE))
        assertEquals(ImeAction.NEXT, imeActionFor(EditorInfo.IME_ACTION_NEXT))
        assertEquals(ImeAction.NEWLINE, imeActionFor(EditorInfo.IME_ACTION_UNSPECIFIED))
        assertEquals(ImeAction.NEWLINE, imeActionFor(EditorInfo.IME_ACTION_SEND or EditorInfo.IME_FLAG_NO_ENTER_ACTION))
    }

    @Test
    fun multiLineIsOnlyForMultiLineText() {
        assertTrue(isMultiLine(text or InputType.TYPE_TEXT_FLAG_MULTI_LINE))
        assertFalse(isMultiLine(text))
    }
}
