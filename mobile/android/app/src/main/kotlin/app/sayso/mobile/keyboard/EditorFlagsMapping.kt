package app.sayso.mobile.keyboard

import android.text.InputType
import android.view.inputmethod.EditorInfo
import app.sayso.mobile.engine.EditorFlags
import app.sayso.mobile.engine.InputKind

enum class ImeAction(val editorAction: Int, val label: String) {
    NEWLINE(EditorInfo.IME_ACTION_NONE, "return"),
    GO(EditorInfo.IME_ACTION_GO, "go"),
    SEARCH(EditorInfo.IME_ACTION_SEARCH, "search"),
    SEND(EditorInfo.IME_ACTION_SEND, "send"),
    NEXT(EditorInfo.IME_ACTION_NEXT, "next"),
    DONE(EditorInfo.IME_ACTION_DONE, "done"),
    PREVIOUS(EditorInfo.IME_ACTION_PREVIOUS, "back"),
}

/** Maps an editor's inputType and imeOptions to the flags the engine needs. */
fun editorFlagsFor(inputType: Int, imeOptions: Int): EditorFlags {
    val typeClass = inputType and InputType.TYPE_MASK_CLASS
    val variation = inputType and InputType.TYPE_MASK_VARIATION
    val kind = when (typeClass) {
        InputType.TYPE_CLASS_NUMBER ->
            if (variation == InputType.TYPE_NUMBER_VARIATION_PASSWORD) InputKind.PASSWORD else InputKind.NUMBER
        InputType.TYPE_CLASS_PHONE -> InputKind.PHONE
        InputType.TYPE_CLASS_DATETIME -> InputKind.NUMBER
        InputType.TYPE_CLASS_TEXT -> when (variation) {
            InputType.TYPE_TEXT_VARIATION_PASSWORD,
            InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD,
            InputType.TYPE_TEXT_VARIATION_WEB_PASSWORD -> InputKind.PASSWORD
            InputType.TYPE_TEXT_VARIATION_EMAIL_ADDRESS,
            InputType.TYPE_TEXT_VARIATION_WEB_EMAIL_ADDRESS -> InputKind.EMAIL
            InputType.TYPE_TEXT_VARIATION_URI -> InputKind.URI
            else -> InputKind.TEXT
        }
        else -> InputKind.TEXT
    }
    val noLearning = imeOptions and EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING != 0
    val textField = typeClass == InputType.TYPE_CLASS_TEXT && kind == InputKind.TEXT
    val filter = variation == InputType.TYPE_TEXT_VARIATION_FILTER
    val noSuggestions = inputType and InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS != 0
    val capFlags = InputType.TYPE_TEXT_FLAG_CAP_SENTENCES or InputType.TYPE_TEXT_FLAG_CAP_WORDS or InputType.TYPE_TEXT_FLAG_CAP_CHARACTERS
    return EditorFlags(
        privacy = noLearning || kind == InputKind.PASSWORD,
        autoCapitalize = textField && inputType and capFlags != 0,
        inputKind = kind,
        suggestionsAllowed = textField && !filter && !noSuggestions,
    )
}

fun imeActionFor(imeOptions: Int): ImeAction {
    if (imeOptions and EditorInfo.IME_FLAG_NO_ENTER_ACTION != 0) return ImeAction.NEWLINE
    return when (imeOptions and EditorInfo.IME_MASK_ACTION) {
        EditorInfo.IME_ACTION_GO -> ImeAction.GO
        EditorInfo.IME_ACTION_SEARCH -> ImeAction.SEARCH
        EditorInfo.IME_ACTION_SEND -> ImeAction.SEND
        EditorInfo.IME_ACTION_NEXT -> ImeAction.NEXT
        EditorInfo.IME_ACTION_DONE -> ImeAction.DONE
        EditorInfo.IME_ACTION_PREVIOUS -> ImeAction.PREVIOUS
        else -> ImeAction.NEWLINE
    }
}

/** True when the editor is a multi-line text field, where enter inserts a newline. */
fun isMultiLine(inputType: Int): Boolean =
    inputType and InputType.TYPE_MASK_CLASS == InputType.TYPE_CLASS_TEXT &&
        inputType and InputType.TYPE_TEXT_FLAG_MULTI_LINE != 0
