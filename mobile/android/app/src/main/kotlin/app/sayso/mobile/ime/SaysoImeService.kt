package app.sayso.mobile.ime

import android.inputmethodservice.InputMethodService
import android.view.KeyEvent
import android.view.View
import android.view.ViewGroup
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputMethodManager
import android.widget.FrameLayout
import android.widget.LinearLayout
import android.widget.Toast
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import app.sayso.mobile.R
import app.sayso.mobile.engine.EditorFlags
import app.sayso.mobile.engine.InputKind
import app.sayso.mobile.engine.SaysoEngine
import app.sayso.mobile.engine.TypingEngine
import app.sayso.mobile.keyboard.EditorSession
import app.sayso.mobile.keyboard.ImeAction
import app.sayso.mobile.keyboard.Key
import app.sayso.mobile.keyboard.KeyCodes
import app.sayso.mobile.keyboard.Keyboard
import app.sayso.mobile.keyboard.KeyboardMode
import app.sayso.mobile.keyboard.ShiftState
import app.sayso.mobile.keyboard.TextEditor
import app.sayso.mobile.keyboard.editorFlagsFor
import app.sayso.mobile.keyboard.imeActionFor
import app.sayso.mobile.keyboard.isMultiLine
import app.sayso.mobile.settings.KeyboardSettings
import app.sayso.mobile.settings.KeyboardSettingsStore

class SaysoImeService : InputMethodService(), KeyboardActionListener {
    private lateinit var engine: TypingEngine
    private lateinit var settingsStore: KeyboardSettingsStore
    private var settings = KeyboardSettings()

    private lateinit var keyboardView: KeyboardView
    private lateinit var strip: SuggestionStripView
    private lateinit var diagnosticsPanel: DiagnosticsPanelView
    private lateinit var emojiPanel: EmojiPanelView

    private val shift = ShiftState()
    private var session: EditorSession? = null
    private var flags = editorFlagsFor(0, 0)
    private var imeAction = ImeAction.NEWLINE
    private var multiLine = false
    private var inputType = 0
    private var revertWord: String? = null

    private val editor = object : TextEditor {
        override fun textBeforeCursor(length: Int): String =
            currentInputConnection?.getTextBeforeCursor(length, 0)?.toString().orEmpty()

        override fun deleteBefore(length: Int) {
            currentInputConnection?.deleteSurroundingText(length, 0)
        }

        override fun insert(text: String) {
            currentInputConnection?.commitText(text, 1)
        }

        override fun batch(block: () -> Unit) {
            val ic = currentInputConnection
            ic?.beginBatchEdit()
            try {
                block()
            } finally {
                ic?.endBatchEdit()
            }
        }
    }

    override fun onCreate() {
        super.onCreate()
        engine = SaysoEngine.get(this)
        settingsStore = KeyboardSettingsStore(this)
    }

    override fun onCreateInputView(): View {
        keyboardView = KeyboardView(this, this)
        strip = SuggestionStripView(this, onPick = ::pick, onRevert = ::revert, onMic = {
            Toast.makeText(this, R.string.voice_coming_soon, Toast.LENGTH_SHORT).show()
        })
        diagnosticsPanel = DiagnosticsPanelView(this)
        emojiPanel = EmojiPanelView(this, onEmoji = ::onText, onBackspace = ::onBackspace, onLetters = { showEmoji(false) })

        val keys = FrameLayout(this).apply {
            clipChildren = false
            addView(keyboardView, FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT))
            addView(emojiPanel, FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0))
        }
        emojiPanel.visibility = View.GONE
        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            clipChildren = false
            addView(diagnosticsPanel)
            addView(strip)
            addView(keys)
        }
        ViewCompat.setOnApplyWindowInsetsListener(root) { v, insets ->
            v.setPadding(0, 0, 0, insets.getInsets(WindowInsetsCompat.Type.navigationBars()).bottom)
            insets
        }
        return root
    }

    override fun onEvaluateFullscreenMode(): Boolean = false

    override fun onStartInputView(info: EditorInfo, restarting: Boolean) {
        super.onStartInputView(info, restarting)
        settings = settingsStore.load()
        inputType = info.inputType
        flags = editorFlagsFor(info.inputType, info.imeOptions)
        imeAction = imeActionFor(info.imeOptions)
        multiLine = isMultiLine(info.inputType)
        engine.setEditorFlags(flags)
        engine.setPreferences(settings.enginePreferences())

        applyPalette(KeyboardPalette.resolve(settings.theme, resources.configuration))
        keyboardView.hapticsEnabled = settings.haptics
        keyboardView.enterLabel = imeAction.label
        keyboardView.mode = initialMode(flags)
        diagnosticsPanel.visibility = if (settings.diagnostics) View.VISIBLE else View.GONE
        if (!settings.diagnostics) keyboardView.diagnostics = null
        showEmoji(false)

        session = EditorSession(editor, info.initialSelStart.coerceAtLeast(0))
        engine.setContext(editor.textBeforeCursor(CONTEXT_LENGTH))
        shift.reset()
        revertWord = null
        afterEdit()
    }

    override fun onUpdateSelection(oldSelStart: Int, oldSelEnd: Int, newSelStart: Int, newSelEnd: Int, candidatesStart: Int, candidatesEnd: Int) {
        super.onUpdateSelection(oldSelStart, oldSelEnd, newSelStart, newSelEnd, candidatesStart, candidatesEnd)
        val s = session ?: return
        if (!s.movedExternally(newSelStart, newSelEnd)) return
        s.resetCursor(newSelStart)
        engine.setContext(editor.textBeforeCursor(CONTEXT_LENGTH))
        revertWord = null
        afterEdit()
    }

    private fun initialMode(flags: EditorFlags) = when (flags.inputKind) {
        InputKind.NUMBER, InputKind.PHONE -> KeyboardMode.SYMBOLS
        else -> KeyboardMode.LETTERS
    }

    private fun applyPalette(palette: KeyboardPalette) {
        keyboardView.palette = palette
        strip.palette = palette
        diagnosticsPanel.palette = palette
        emojiPanel.palette = palette
        (keyboardView.parent?.parent as? View)?.setBackgroundColor(palette.ground)
    }

    // KeyboardActionListener

    override fun onLetter(key: Key, x: Float, y: Float, timeMs: Long) {
        val code = engine.onTouch(x, y, timeMs, shift.isUpperCase)
        if (code == TypingEngine.NO_KEY) return
        val char = code.toChar()
        session?.insert((if (shift.isUpperCase) char.uppercaseChar() else char).toString())
        shift.onLetter()
        revertWord = null
        afterEdit()
    }

    override fun onText(text: String) {
        if (text.length == 1 && text[0] in SEPARATORS) {
            commitSeparator(text[0])
            return
        }
        engine.onText(text)
        session?.insert(text)
        if (text.any { it.isLetter() }) shift.onLetter()
        revertWord = null
        afterEdit()
    }

    override fun onSpace() = commitSeparator(' ')

    override fun onSpaceLongPress() {
        getSystemService(InputMethodManager::class.java).showInputMethodPicker()
    }

    override fun onBackspace() {
        val s = session ?: return
        if (revertWord != null) {
            // Backspace right after an autocorrect restores what was typed, without the separator.
            revert()
            s.deleteCharBefore()
            engine.setContext(editor.textBeforeCursor(CONTEXT_LENGTH))
            afterEdit()
            return
        }
        val selected = currentInputConnection?.getSelectedText(0)
        when {
            !selected.isNullOrEmpty() -> {
                currentInputConnection?.commitText("", 1)
                engine.setContext(editor.textBeforeCursor(CONTEXT_LENGTH))
            }
            s.deleteCharBefore() -> engine.onBackspace()
            else -> sendDownUpKeyEvents(KeyEvent.KEYCODE_DEL)
        }
        afterEdit()
    }

    override fun onDeleteWord() {
        session?.deleteWordBefore()
        engine.setContext(editor.textBeforeCursor(CONTEXT_LENGTH))
        revertWord = null
        afterEdit()
    }

    override fun onCursorMove(steps: Int) {
        val code = if (steps > 0) KeyEvent.KEYCODE_DPAD_RIGHT else KeyEvent.KEYCODE_DPAD_LEFT
        repeat(kotlin.math.abs(steps)) { sendDownUpKeyEvents(code) }
    }

    override fun onSpecialKey(code: Int, timeMs: Long) {
        when (code) {
            KeyCodes.SHIFT -> {
                shift.onShiftTap(timeMs)
                keyboardView.shift = shift.state
            }
            KeyCodes.SYMBOLS -> keyboardView.mode = KeyboardMode.SYMBOLS
            KeyCodes.SYMBOLS_MORE -> keyboardView.mode = KeyboardMode.SYMBOLS_MORE
            KeyCodes.LETTERS -> {
                keyboardView.mode = KeyboardMode.LETTERS
                afterEdit()
            }
            KeyCodes.EMOJI -> showEmoji(true)
            KeyCodes.ENTER -> onEnter()
        }
    }

    override fun onKeyboardLaidOut(keyboard: Keyboard) {
        if (keyboard.mode == KeyboardMode.LETTERS) engine.setLayout(keyboard.engineLayout())
        if (settings.diagnostics) refreshStrip()
    }

    private fun onEnter() {
        when {
            imeAction != ImeAction.NEWLINE -> currentInputConnection?.performEditorAction(imeAction.editorAction)
            multiLine -> commitSeparator('\n')
            else -> sendDownUpKeyEvents(KeyEvent.KEYCODE_ENTER)
        }
        if (imeAction != ImeAction.NEWLINE || !multiLine) engine.setContext(editor.textBeforeCursor(CONTEXT_LENGTH))
    }

    private fun commitSeparator(char: Char) {
        val action = engine.onSpaceOrPunct(char)
        session?.apply(action)
        revertWord = if (action.wasAutocorrect) action.original else null
        afterEdit()
    }

    private fun pick(index: Int) {
        session?.apply(engine.pickSuggestion(index))
        revertWord = null
        afterEdit()
    }

    private fun revert() {
        val action = engine.revertLastAutocorrect() ?: return
        session?.apply(action)
        revertWord = null
        afterEdit()
    }

    private fun showEmoji(show: Boolean) {
        if (show) emojiPanel.layoutParams = emojiPanel.layoutParams.apply { height = keyboardView.height }
        emojiPanel.visibility = if (show) View.VISIBLE else View.GONE
        keyboardView.visibility = if (show) View.INVISIBLE else View.VISIBLE
    }

    private fun afterEdit() {
        if (flags.autoCapitalize && keyboardView.mode == KeyboardMode.LETTERS) {
            val caps = (currentInputConnection?.getCursorCapsMode(inputType) ?: 0) != 0
            shift.onCapsMode(caps)
        }
        keyboardView.shift = shift.state
        refreshStrip()
    }

    private fun refreshStrip() {
        strip.show(engine.suggestions(), revertWord)
        if (settings.diagnostics) {
            val d = engine.diagnostics()
            keyboardView.diagnostics = d
            diagnosticsPanel.show(d)
        }
    }

    private companion object {
        const val CONTEXT_LENGTH = 64
        val SEPARATORS = setOf(' ', '.', ',', '!', '?', ';', ':', ')', '"', '\n')
    }
}
