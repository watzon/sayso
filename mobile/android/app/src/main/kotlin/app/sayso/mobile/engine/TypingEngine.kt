package app.sayso.mobile.engine

/**
 * The boundary between the keyboard UI and the typing core.
 *
 * The UI owns drawing, gestures, and the editor connection. The engine owns
 * every typing decision: which key a touch meant, what the current word is,
 * suggestions, autocorrect, and what it learns. Coordinates are pixels in the
 * keyboard view, origin at its top left.
 *
 * All calls come from the main thread.
 */
interface TypingEngine {
    /** The keys the engine may resolve touches to. Sent again whenever the keyboard is laid out. */
    fun setLayout(layout: EngineLayout)

    fun setEditorFlags(flags: EditorFlags)

    fun setPreferences(preferences: EnginePreferences)

    /**
     * The editor changed in a way the engine did not cause (new field, cursor
     * moved, text deleted). The engine rebuilds its state, including the
     * current word, from [textBeforeCursor].
     */
    fun setContext(textBeforeCursor: String)

    /**
     * A touch on a letter key. Returns the code of the key the engine chose,
     * which may differ from the key under the finger. [shifted] tells the
     * engine to record the upper-case letter in the current word.
     */
    fun onTouch(x: Float, y: Float, timeMs: Long, shifted: Boolean): Int

    /**
     * Text that did not come from a letter touch and is not a space or
     * punctuation: digits, accents, apostrophes, emoji. Returns what to write,
     * usually `CommitAction(0, text)`; an engine may change the word before it.
     */
    fun onText(text: String): CommitAction

    /**
     * The delete key, before anything is deleted. Null: delete one character
     * before the cursor. Otherwise apply the action instead: right after an
     * autocorrect it brings back the typed word, without its separator, as the
     * word being typed.
     */
    fun onBackspace(): CommitAction?

    /** A space or punctuation ends the current word. The UI applies the returned action. */
    fun onSpaceOrPunct(char: Char): CommitAction

    /** Up to three candidates for the current word, or for the next word when none is in progress. */
    fun suggestions(): List<Suggestion>

    fun pickSuggestion(index: Int): CommitAction

    /**
     * Undo the last autocorrect from the suggestion strip: the typed word and
     * its separator come back, and the word is not corrected again. Null when
     * there is nothing to undo.
     */
    fun revertLastAutocorrect(): CommitAction?

    fun personalWords(): List<String>
    fun addPersonalWord(word: String)
    fun removePersonalWord(word: String)

    fun replacements(): List<Replacement>
    fun putReplacement(replacement: Replacement)
    fun removeReplacement(shortcut: String)

    /** Forget learned touch offsets and word history. Personal words and replacements added by hand stay. */
    fun resetLearnedData()

    fun diagnostics(): Diagnostics

    companion object {
        /** What [onTouch] returns when the layout has no keys. */
        const val NO_KEY = 0
    }
}

/** A key the engine can resolve to, in keyboard view pixels. */
data class EngineKey(
    val code: Int,
    val label: String,
    val left: Float,
    val top: Float,
    val right: Float,
    val bottom: Float,
) {
    val centerX: Float get() = (left + right) / 2f
    val centerY: Float get() = (top + bottom) / 2f
}

data class EngineLayout(val width: Float, val height: Float, val keys: List<EngineKey>)

enum class InputKind { TEXT, EMAIL, URI, NUMBER, PHONE, PASSWORD }

data class EditorFlags(
    /** Learn nothing and suggest nothing personal: passwords, incognito, no-personalized-learning fields. */
    val privacy: Boolean,
    /** The field wants sentence capitalization at the cursor. */
    val autoCapitalize: Boolean,
    val inputKind: InputKind,
    /** Autocorrect and suggestions make sense here (not a password, URL, email, or number). */
    val suggestionsAllowed: Boolean,
)

enum class AutocorrectMode { OFF, NORMAL, STRICT }

data class EnginePreferences(
    val autocorrect: AutocorrectMode = AutocorrectMode.NORMAL,
    val nextWordPrediction: Boolean = true,
)

/**
 * Delete [deleteBefore] characters before the cursor, then insert [text].
 * When [wasAutocorrect] is true, [original] is the text that was replaced, and
 * the UI offers to revert it.
 */
data class CommitAction(
    val deleteBefore: Int,
    val text: String,
    val wasAutocorrect: Boolean = false,
    val original: String = "",
) {
    /** This action followed by [next], as one action that is not an autocorrect. */
    fun then(next: CommitAction): CommitAction {
        val fromText = minOf(next.deleteBefore, text.length)
        return CommitAction(deleteBefore + next.deleteBefore - fromText, text.dropLast(fromText) + next.text)
    }
}

data class Suggestion(val text: String, val score: Float, val isAutoCommit: Boolean)

data class Replacement(val shortcut: String, val expansion: String)

data class KeyDiagnostics(
    val code: Int,
    val left: Float,
    val top: Float,
    val right: Float,
    val bottom: Float,
    val learnedDx: Float,
    val learnedDy: Float,
    /** How far the key reaches into its neighbors for the next touch, when the engine reports it. */
    val expansion: Float = 0f,
    /** Confirmed touches that taught the offset, when the engine counts them. */
    val samples: Int = 0,
)

/** One scored candidate for the word being typed (lower scores are better, in nats). */
data class CandidateDiagnostics(
    val text: String,
    val total: Float,
    val spatial: Float,
    val languageModel: Float,
    val edits: Int,
)

data class Diagnostics(
    /** Each key's effective hit area after expansion, and the touch offset learned for it. */
    val keys: List<KeyDiagnostics>,
    val candidates: List<Suggestion>,
    /** Score gap between the best candidate and the next. Autocorrect fires only above its threshold. */
    val autocorrectMargin: Float,
    val autocorrectThreshold: Float,
    /** The scores behind [candidates], when the engine reports them. */
    val details: List<CandidateDiagnostics> = emptyList(),
)

/** Characters that belong to a word, for both the UI and the engine. */
fun isWordChar(c: Char): Boolean = c.isLetterOrDigit() || c == '\''
