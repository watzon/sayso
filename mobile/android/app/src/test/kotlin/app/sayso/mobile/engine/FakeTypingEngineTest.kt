package app.sayso.mobile.engine

import app.sayso.mobile.keyboard.KeyboardLayouts
import app.sayso.mobile.keyboard.KeyboardMode
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

class FakeTypingEngineTest {
    @get:Rule val temp = TemporaryFolder()

    private val layout = KeyboardLayouts.build(KeyboardMode.LETTERS, 1000f, 400f, 10f, 20f).engineLayout()
    private val engine = FakeTypingEngine().apply { setLayout(layout) }

    private fun key(c: Char) = layout.keys.first { it.code == c.code }

    private fun type(word: String, shiftFirst: Boolean = false) {
        word.forEachIndexed { i, c ->
            val k = key(c.lowercaseChar())
            engine.onTouch(k.centerX, k.centerY, i.toLong(), shifted = shiftFirst && i == 0)
        }
    }

    @Test
    fun touchResolvesToTheNearestKey() {
        val q = key('q')
        assertEquals('q'.code, engine.onTouch(q.centerX + 20f, q.centerY - 10f, 0, false))
        // Just past the halfway point to "w".
        assertEquals('w'.code, engine.onTouch(q.centerX + 51f, q.centerY, 0, false))
    }

    @Test
    fun learnedOffsetMovesTheHitTargetAndShowsInDiagnostics() {
        val q = key('q')
        repeat(60) { engine.onTouch(q.centerX + 45f, q.centerY, it.toLong(), false) }
        // Touches always land right of q's center, so q now owns a point past the old midpoint.
        assertEquals('q'.code, engine.onTouch(q.centerX + 55f, q.centerY, 100, false))
        val dq = engine.diagnostics().keys.first { it.code == 'q'.code }
        assertTrue(dq.learnedDx > 30f)
    }

    @Test
    fun privacyFieldsLearnNothing() {
        engine.setEditorFlags(EditorFlags(privacy = true, autoCapitalize = false, inputKind = InputKind.TEXT, suggestionsAllowed = true))
        val q = key('q')
        repeat(20) { engine.onTouch(q.centerX + 45f, q.centerY, it.toLong(), false) }
        assertEquals(0f, engine.diagnostics().keys.first { it.code == 'q'.code }.learnedDx)
    }

    @Test
    fun spaceAutocorrectsAKnownTypoAndKeepsCase() {
        type("Teh", shiftFirst = true)
        assertTrue(engine.suggestions().first().isAutoCommit)
        val action = engine.onSpaceOrPunct(' ')
        assertEquals(CommitAction(3, "The ", wasAutocorrect = true, original = "Teh"), action)
    }

    @Test
    fun knownWordsAreLeftAlone() {
        type("work")
        assertEquals(CommitAction(0, " "), engine.onSpaceOrPunct(' '))
    }

    @Test
    fun revertRestoresTheTypedWordAndItIsNotCorrectedAgain() {
        type("teh")
        engine.onSpaceOrPunct(' ')
        assertEquals(CommitAction(4, "teh "), engine.revertLastAutocorrect())
        assertNull(engine.revertLastAutocorrect())
        type("teh")
        assertFalse(engine.onSpaceOrPunct(' ').wasAutocorrect)
    }

    @Test
    fun autocorrectOffOnlySuggests() {
        engine.setPreferences(EnginePreferences(autocorrect = AutocorrectMode.OFF))
        type("teh")
        assertTrue(engine.suggestions().none { it.isAutoCommit })
        assertEquals(CommitAction(0, " "), engine.onSpaceOrPunct(' '))
    }

    @Test
    fun strictNeedsABiggerMarginThanNormal() {
        // "fo" is close to several words, so the best one wins by a small margin.
        type("fo")
        engine.suggestions()
        val normal = engine.diagnostics()
        assertTrue(normal.autocorrectMargin < 0.35f)
        engine.setContext("")
        engine.setPreferences(EnginePreferences(autocorrect = AutocorrectMode.STRICT))
        type("fo")
        assertTrue(engine.diagnostics().autocorrectThreshold > normal.autocorrectThreshold)
        assertFalse(engine.onSpaceOrPunct(' ').wasAutocorrect)
    }

    @Test
    fun replacementsExpandOnSpace() {
        engine.putReplacement(Replacement("omw", "on my way"))
        type("omw")
        assertEquals(CommitAction(3, "on my way ", wasAutocorrect = true, original = "omw"), engine.onSpaceOrPunct(' '))
    }

    @Test
    fun personalWordsAreNeverCorrected() {
        engine.addPersonalWord("Wyld")
        type("wyld")
        assertFalse(engine.onSpaceOrPunct(' ').wasAutocorrect)
    }

    @Test
    fun pickingASuggestionReplacesThePartialWord() {
        type("tod")
        val index = engine.suggestions().indexOfFirst { it.text == "today" }
        assertEquals(CommitAction(3, "today "), engine.pickSuggestion(index))
    }

    @Test
    fun nextWordPredictionFollowsTheContext() {
        engine.setContext("Thank ")
        assertEquals("you", engine.suggestions().first().text)
        engine.setPreferences(EnginePreferences(nextWordPrediction = false))
        assertTrue(engine.suggestions().isEmpty())
    }

    @Test
    fun setContextPicksUpTheWordBeforeTheCursor() {
        engine.setContext("I said teh")
        assertEquals(CommitAction(3, "the ", wasAutocorrect = true, original = "teh"), engine.onSpaceOrPunct(' '))
    }

    @Test
    fun personalDataSurvivesARestartAndResetKeepsIt() {
        val file = temp.newFile("personal.tsv")
        FakeTypingEngine(file).apply {
            addPersonalWord("Sayso")
            putReplacement(Replacement("brb", "be right back"))
            resetLearnedData()
        }
        val reloaded = FakeTypingEngine(file)
        assertEquals(listOf("Sayso"), reloaded.personalWords())
        assertEquals(listOf(Replacement("brb", "be right back")), reloaded.replacements())
    }
}
