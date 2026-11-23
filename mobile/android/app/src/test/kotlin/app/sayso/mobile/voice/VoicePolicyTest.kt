package app.sayso.mobile.voice

import android.text.InputType
import android.view.inputmethod.EditorInfo
import app.sayso.mobile.engine.CloudAsrProvider
import app.sayso.mobile.engine.CloudConfig
import app.sayso.mobile.engine.CommitAction
import app.sayso.mobile.keyboard.ImeAction
import app.sayso.mobile.keyboard.editorFlagsFor
import app.sayso.mobile.settings.VoiceEngineChoice
import app.sayso.mobile.settings.VoiceSettings
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class VoicePolicyTest {
    private val text = InputType.TYPE_CLASS_TEXT
    private val cloudSettings = VoiceSettings(
        engine = VoiceEngineChoice.CLOUD,
        cloud = CloudConfig(asrProvider = CloudAsrProvider.GROQ, asrApiKey = "k"),
    )

    @Test
    fun passwordAndIncognitoFieldsAreSensitive() {
        val password = VoicePolicy.forEditor(editorFlagsFor(text or InputType.TYPE_TEXT_VARIATION_PASSWORD, 0))
        val webPassword = VoicePolicy.forEditor(editorFlagsFor(text or InputType.TYPE_TEXT_VARIATION_WEB_PASSWORD, 0))
        val pin = VoicePolicy.forEditor(editorFlagsFor(InputType.TYPE_CLASS_NUMBER or InputType.TYPE_NUMBER_VARIATION_PASSWORD, 0))
        val incognito = VoicePolicy.forEditor(editorFlagsFor(text, EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING))
        val plain = VoicePolicy.forEditor(editorFlagsFor(text, 0))
        for (p in listOf(password, webPassword, pin, incognito)) {
            assertTrue(p.sensitive)
            assertFalse(p.cloudAllowed)
        }
        assertFalse(plain.sensitive)
        assertTrue(plain.cloudAllowed)
    }

    @Test
    fun sensitiveFieldsNeverGetTheCloudConfig() {
        assertEquals(CloudConfig.OFF, VoicePolicy(sensitive = true).cloudConfig(cloudSettings))
        assertEquals(cloudSettings.cloud, VoicePolicy(sensitive = false).cloudConfig(cloudSettings))
    }

    private val all = EngineAvailability(local = true, system = true, cloud = true)
    private val open = VoicePolicy(sensitive = false)

    @Test
    fun autoPrefersTheLocalModelThenTheSystemRecognizer() {
        assertEquals(listOf(VoiceEngine.LOCAL, VoiceEngine.SYSTEM), planEngines(VoiceEngineChoice.AUTO, all, open))
    }

    @Test
    fun withNoDownloadVoiceUsesTheSystemRecognizer() {
        val none = EngineAvailability(local = false, system = true, cloud = false)
        assertEquals(listOf(VoiceEngine.SYSTEM), planEngines(VoiceEngineChoice.AUTO, none, open))
        assertEquals(listOf(VoiceEngine.SYSTEM), planEngines(VoiceEngineChoice.CLOUD, none, open))
    }

    @Test
    fun cloudComesFirstOnlyWhenChosenAndAllowed() {
        assertEquals(listOf(VoiceEngine.CLOUD, VoiceEngine.LOCAL, VoiceEngine.SYSTEM), planEngines(VoiceEngineChoice.CLOUD, all, open))
        assertEquals(
            "never cloud in a sensitive field",
            listOf(VoiceEngine.LOCAL, VoiceEngine.SYSTEM),
            planEngines(VoiceEngineChoice.CLOUD, all, VoicePolicy(sensitive = true)),
        )
        assertFalse("auto never picks cloud", VoiceEngine.CLOUD in planEngines(VoiceEngineChoice.AUTO, all, open))
    }

    @Test
    fun systemChoiceFallsBackToTheLocalModel() {
        assertEquals(listOf(VoiceEngine.SYSTEM, VoiceEngine.LOCAL), planEngines(VoiceEngineChoice.SYSTEM, all, open))
        assertEquals(
            listOf(VoiceEngine.LOCAL),
            planEngines(VoiceEngineChoice.SYSTEM, all.copy(system = false), open),
        )
    }

    @Test
    fun whenTheAsrProcessDiesTheNextEngineTakesOver() {
        val plan = planEngines(VoiceEngineChoice.AUTO, all, open)
        assertEquals(VoiceEngine.SYSTEM, fallbackAfter(VoiceEngine.LOCAL, plan))
        assertNull(fallbackAfter(VoiceEngine.SYSTEM, plan))
        assertNull(fallbackAfter(VoiceEngine.CLOUD, plan))
    }

    @Test
    fun noEngineAtAll() {
        assertEquals(emptyList<VoiceEngine>(), planEngines(VoiceEngineChoice.AUTO, EngineAvailability(false, false, true), open))
    }

    @Test
    fun handsFreeSendsOnlyWhereTheFieldHasAnAction() {
        assertTrue(handsFreeSends(ImeAction.SEND))
        assertTrue(handsFreeSends(ImeAction.SEARCH))
        assertFalse(handsFreeSends(ImeAction.NEWLINE))
    }
}

class VoiceUndoTest {
    @Test
    fun undoRemovesExactlyTheInsertedText() {
        val u = VoiceUndo()
        u.record(" hello world.")
        assertEquals(CommitAction(13, ""), u.undo("Say hello world.".takeLast(13)))
        assertNull("one tap only", u.inserted)
    }

    @Test
    fun undoDoesNothingOnceTheTextChanged() {
        val u = VoiceUndo()
        u.record(" hello world.")
        assertNull(u.undo("ello world!!!"))
        assertNull(u.inserted)
    }

    @Test
    fun nothingToUndo() {
        val u = VoiceUndo()
        assertNull(u.undo("anything"))
        u.record("")
        assertNull(u.inserted)
        u.record("x")
        u.clear()
        assertNull(u.undo("x"))
    }
}

class PcmTest {
    @Test
    fun levelMapsSilenceToZeroAndLoudSpeechToOne() {
        assertEquals(0f, Pcm.level(ShortArray(320)), 0f)
        assertEquals(1f, Pcm.level(ShortArray(320) { if (it % 2 == 0) 20_000 else -20_000 }), 0f)
        val mid = Pcm.level(ShortArray(320) { if (it % 2 == 0) 1_000 else -1_000 }) // about -30 dBFS
        assertTrue(mid in 0.4f..0.7f)
        assertEquals(0f, Pcm.level(ShortArray(0)), 0f)
    }

    @Test
    fun systemRmsLevel() {
        assertEquals(0f, Pcm.systemRmsLevel(-2f), 0f)
        assertEquals(1f, Pcm.systemRmsLevel(12f), 0f)
        assertEquals(0.5f, Pcm.systemRmsLevel(4f), 0.001f)
    }

    @Test
    fun bufferStopsAtItsLimit() {
        val b = PcmBuffer(maxSamples = 40_000)
        assertTrue(b.append(ShortArray(30_000) { 1 }))
        assertFalse(b.append(ShortArray(20_000) { 2 }))
        assertEquals(40_000, b.size)
        val all = b.toShortArray()
        assertEquals(1, all[29_999].toInt())
        assertEquals(2, all[39_999].toInt())
        assertFalse(b.append(ShortArray(1)))
        assertEquals(2_500L, Pcm.durationMs(40_000))
    }

    @Test
    fun capitalsFromTheStreamingModelBecomeSentenceCase() {
        assertEquals("Hello I think I'm sure", normalizeTranscript("HELLO I THINK I'M SURE"))
        assertEquals("I'm here and I agree", normalizeTranscript(" I'M  HERE AND I AGREE "))
        assertEquals("Already mixed Case", normalizeTranscript("Already mixed Case"))
        assertEquals("", normalizeTranscript("   "))
        assertEquals("123", normalizeTranscript("123"))
    }
}
