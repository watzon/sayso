package app.sayso.mobile.voice

import app.sayso.mobile.engine.CloudConfig
import app.sayso.mobile.engine.CloudStyleProvider
import app.sayso.mobile.engine.CommitAction
import app.sayso.mobile.engine.EditorFlags
import app.sayso.mobile.engine.VoiceStyle
import app.sayso.mobile.keyboard.ImeAction
import app.sayso.mobile.settings.VoiceEngineChoice
import app.sayso.mobile.settings.VoiceSettings

/**
 * What a field allows. Password fields, incognito fields
 * (IME_FLAG_NO_PERSONALIZED_LEARNING) and the rest of [EditorFlags.privacy]
 * are sensitive: their audio and text never go to a cloud provider, and
 * nothing is written to voice history.
 */
data class VoicePolicy(val sensitive: Boolean) {
    val cloudAllowed: Boolean get() = !sensitive

    /** The cloud config for this field: off in sensitive fields, whatever the settings say. */
    fun cloudConfig(settings: VoiceSettings): CloudConfig = if (cloudAllowed) settings.cloud else CloudConfig.OFF

    companion object {
        fun forEditor(flags: EditorFlags) = VoicePolicy(sensitive = flags.privacy)
    }
}

/** Which engines can run right now. */
data class EngineAvailability(
    /** A local model is installed and active, the build has sherpa-onnx, and :asr has not failed. */
    val local: Boolean,
    /** SpeechRecognizer has a recognition service on this device. */
    val system: Boolean,
    /** Cloud transcription has a provider and a key. */
    val cloud: Boolean,
)

/**
 * The engines to try, best first. The first one starts the dictation; when it
 * fails, the next one takes over. The system recognizer is the last resort of
 * every choice, so voice works with no download at all.
 */
fun planEngines(choice: VoiceEngineChoice, available: EngineAvailability, policy: VoicePolicy): List<VoiceEngine> {
    val order = when (choice) {
        VoiceEngineChoice.AUTO -> listOf(VoiceEngine.LOCAL, VoiceEngine.SYSTEM)
        VoiceEngineChoice.SYSTEM -> listOf(VoiceEngine.SYSTEM, VoiceEngine.LOCAL)
        VoiceEngineChoice.CLOUD -> listOf(VoiceEngine.CLOUD, VoiceEngine.LOCAL, VoiceEngine.SYSTEM)
    }
    return order.filter {
        when (it) {
            VoiceEngine.LOCAL -> available.local
            VoiceEngine.SYSTEM -> available.system
            VoiceEngine.CLOUD -> available.cloud && policy.cloudAllowed
        }
    }
}

/** The engine after [failed] in [plan], or null when none is left. */
fun fallbackAfter(failed: VoiceEngine, plan: List<VoiceEngine>): VoiceEngine? =
    plan.dropWhile { it != failed }.drop(1).firstOrNull()

/**
 * True when [style] rewrites the text: it has a prompt and [cloud] (already
 * passed through [VoicePolicy.cloudConfig]) has a style provider. Otherwise
 * only the text rules run, and the keyboard does not name a style.
 */
fun styleApplies(style: VoiceStyle?, cloud: CloudConfig): Boolean =
    style != null && style.prompt.isNotBlank() && cloud.styleProvider != CloudStyleProvider.OFF

/** In hands-free mode Done sends with the field's action. A field whose Enter is a newline has nothing to send. */
fun handsFreeSends(action: ImeAction): Boolean = action != ImeAction.NEWLINE

/**
 * The one-tap undo after a dictation: removes exactly the text it inserted,
 * and only while that text is still right before the cursor.
 */
class VoiceUndo {
    var inserted: String? = null
        private set

    fun record(text: String) {
        inserted = text.ifEmpty { null }
    }

    fun clear() {
        inserted = null
    }

    /** The edit that removes the dictation, given the [inserted].length characters before the cursor; null when they no longer match. */
    fun undo(textBeforeCursor: String): CommitAction? {
        val text = inserted ?: return null
        inserted = null
        return if (textBeforeCursor.endsWith(text)) CommitAction(text.length, "") else null
    }
}
