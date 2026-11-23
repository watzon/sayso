package app.sayso.mobile.engine

import app.sayso.mobile.keyboard.KeyboardLayouts
import app.sayso.mobile.keyboard.KeyboardMode
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File

/**
 * NativeTypingEngine through the real JNI glue (sayso_jni.c) and the real Go
 * core, built for this computer by mobile/scripts/build-android.sh and passed
 * in with -PsaysoHostLibDir. Without it these tests are skipped.
 */
class NativeTypingEngineTest {
    @get:Rule val temp = TemporaryFolder()

    private val layout = KeyboardLayouts.build(KeyboardMode.LETTERS, 1000f, 400f, 10f, 20f).engineLayout()
    private val model = System.getProperty("sayso.model").orEmpty()
    private lateinit var journal: File
    private lateinit var engine: NativeTypingEngine

    /** The text field: what the IME's EditorSession would hold. */
    private val field = StringBuilder()

    @Before
    fun open() {
        assumeTrue("host JNI libraries not built (-PsaysoHostLibDir)", System.getProperty("sayso.hostLibs") == "true")
        journal = File(temp.root, "typing.journal")
        engine = reopen()
    }

    @After
    fun close() {
        if (::engine.isInitialized) engine.close()
    }

    private fun reopen(): NativeTypingEngine = NativeTypingEngine.open(model, journal.path).apply {
        setLayout(layout)
        setEditorFlags(TEXT)
        setContext(field.toString())
    }

    private fun apply(a: CommitAction) {
        field.setLength(field.length - minOf(a.deleteBefore, field.length))
        field.append(a.text)
    }

    /** Taps key centers for letters; everything else goes the way the IME sends it. */
    private fun type(s: String) {
        for (c in s) {
            when {
                c.lowercaseChar() in 'a'..'z' -> {
                    val k = layout.keys.first { it.code == c.lowercaseChar().code }
                    val code = engine.onTouch(k.centerX, k.centerY, 0, c.isUpperCase())
                    assertTrue("no letter for $c", code != TypingEngine.NO_KEY)
                    field.appendCodePoint(code)
                }
                c in SEPARATORS -> apply(engine.onSpaceOrPunct(c))
                else -> apply(engine.onText(c.toString()))
            }
        }
    }

    private fun backspace() {
        val a = engine.onBackspace()
        if (a != null) apply(a) else field.setLength(field.length - 1)
    }

    @Test
    fun touchesTypeLettersInTheirCase() {
        type("Hello")
        assertEquals("Hello", field.toString())
    }

    @Test
    fun spaceAutocorrectsAndReportsTheTypedWord() {
        type("I want teh")
        val a = engine.onSpaceOrPunct(' ')
        assertEquals(CommitAction(3, "the ", wasAutocorrect = true, original = "teh"), a)
    }

    @Test
    fun backspaceAfterAnAutocorrectBringsBackTheWordAndKeepsIt() {
        type("I want teh ")
        assertEquals("I want the ", field.toString())
        backspace()
        assertEquals("I want teh", field.toString())
        type(" ")
        assertEquals("I want teh ", field.toString())
    }

    @Test
    fun revertFromTheStripPutsBackTheWordAndItsSeparator() {
        type("I want teh")
        val a = engine.onSpaceOrPunct(',')
        apply(a)
        assertEquals("I want the,", field.toString())
        val revert = engine.revertLastAutocorrect()!!
        apply(revert)
        assertEquals("I want teh,", field.toString())
        assertNull(engine.revertLastAutocorrect())
        type(" now ")
        assertEquals("I want teh, now ", field.toString())
    }

    @Test
    fun suggestionsAndPick() {
        type("I want to beau")
        val sugs = engine.suggestions()
        assertTrue(sugs.isNotEmpty() && sugs.size <= 3)
        val i = sugs.indexOfFirst { it.text.startsWith("beaut") }
        assertTrue("no completion in $sugs", i >= 0)
        apply(engine.pickSuggestion(i))
        assertEquals("I want to ${sugs[i].text} ", field.toString())
        type(".")
        assertEquals("I want to ${sugs[i].text}. ", field.toString())
    }

    @Test
    fun preferences() {
        engine.setPreferences(EnginePreferences(autocorrect = AutocorrectMode.OFF))
        type("I want teh")
        assertTrue(engine.suggestions().none { it.isAutoCommit })
        type(" ")
        assertEquals("I want teh ", field.toString())

        engine.setPreferences(EnginePreferences(autocorrect = AutocorrectMode.STRICT))
        field.setLength(0)
        engine.setContext("")
        type("I want cna ")
        assertEquals("I want cna ", field.toString())

        engine.setPreferences(EnginePreferences(nextWordPrediction = false))
        engine.setContext("Thank ")
        assertTrue(engine.suggestions().isEmpty())
        engine.setPreferences(EnginePreferences(nextWordPrediction = true))
        assertTrue(engine.suggestions().isNotEmpty())
    }

    @Test
    fun replacementsCountUtf16WhenUndone() {
        engine.putReplacement(Replacement("Omw ", "on my way 🚗"))
        assertEquals(listOf(Replacement("omw", "on my way 🚗")), engine.replacements())
        type("omw ")
        assertEquals("on my way 🚗 ", field.toString())
        val undo = engine.onBackspace()!!
        assertEquals("on my way 🚗 ".length, undo.deleteBefore)
        apply(undo)
        assertEquals("omw", field.toString())
        engine.removeReplacement("omw")
        assertTrue(engine.replacements().isEmpty())
    }

    @Test
    fun onTextKeepsDigitsAndEmojiInPlace() {
        type("route 66")
        apply(engine.onText("😀"))
        type(" ok")
        assertEquals("route 66😀 ok", field.toString())
    }

    @Test
    fun dictionaryEditsPersistEvenAfterAPrivateField() {
        engine.setEditorFlags(TEXT.copy(privacy = true))
        engine.addPersonalWord("Zorblax")
        engine.putReplacement(Replacement("brb", "be right back"))
        engine.close()
        engine = reopen()
        assertEquals(listOf("Zorblax"), engine.personalWords())
        assertEquals(listOf(Replacement("brb", "be right back")), engine.replacements())
        engine.removePersonalWord("Zorblax")
        assertTrue(engine.personalWords().isEmpty())
    }

    @Test
    fun privateFieldsLearnNothing() {
        engine.setEditorFlags(TEXT.copy(privacy = true))
        type("flurbo is here. flurbo is here. ")
        assertTrue(engine.personalWords().isEmpty())
        engine.setEditorFlags(TEXT)
        type("flurbo is here. flurbo is here. ")
        assertEquals(listOf("flurbo"), engine.personalWords())
    }

    @Test
    fun resetForgetsLearnedWordsButKeepsWhatWasAdded() {
        engine.addPersonalWord("Sayso")
        engine.putReplacement(Replacement("brb", "be right back"))
        type("flurbo is here. flurbo is here. ")
        assertEquals(listOf("Sayso", "flurbo"), engine.personalWords())
        engine.resetLearnedData()
        assertEquals(listOf("Sayso"), engine.personalWords())
        assertEquals(listOf(Replacement("brb", "be right back")), engine.replacements())
        engine.close()
        engine = reopen()
        assertEquals(listOf("Sayso"), engine.personalWords())
    }

    @Test
    fun diagnosticsReportKeysCandidatesAndMargin() {
        type("I want th")
        engine.suggestions()
        val d = engine.diagnostics()
        assertEquals(layout.keys.size, d.keys.size)
        val q = layout.keys.first { it.code == 'q'.code }
        val dq = d.keys.first { it.code == 'q'.code }
        assertTrue(dq.left <= q.left && dq.right >= q.right)
        assertTrue(d.details.isNotEmpty())
        assertEquals(d.details.size, d.candidates.size)
        assertEquals(0f, d.autocorrectThreshold)
        assertFalse(d.autocorrectMargin.isNaN())
    }

    private companion object {
        val TEXT = EditorFlags(privacy = false, autoCapitalize = true, inputKind = InputKind.TEXT, suggestionsAllowed = true)
        val SEPARATORS = setOf(' ', '.', ',', '!', '?', ';', ':', ')', '"', '\n')
    }
}
