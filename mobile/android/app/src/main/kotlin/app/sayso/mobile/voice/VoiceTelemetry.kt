package app.sayso.mobile.voice

import app.sayso.mobile.engine.CloudAsrProvider
import app.sayso.mobile.engine.CloudConfig
import app.sayso.mobile.telemetry.DictationSignal

/** Cloud models with public names, by the desktop's provider kinds; any other model is sent as its provider only. */
private val KNOWN_CLOUD_MODELS = mapOf(
    CloudAsrProvider.OPENAI_WHISPER to ("open_ai" to setOf("gpt-4o-transcribe", "gpt-4o-mini-transcribe", "gpt-transcribe", "whisper-1")),
    CloudAsrProvider.GROQ to ("groq" to setOf("whisper-large-v3-turbo", "whisper-large-v3")),
    CloudAsrProvider.OPENAI_COMPATIBLE to ("open_ai_compatible" to emptySet()),
)

/**
 * What dictation.completed tells about a dictation: the engine's model by a
 * public name only, and whether a cloud style rewrote it. A custom
 * endpoint's model or a model the user typed in stays out.
 */
fun dictationSignal(engine: VoiceEngine, localModelId: String?, cloud: CloudConfig, styled: Boolean): DictationSignal {
    val style = if (styled) "ai_cloud" else "raw"
    return when (engine) {
        VoiceEngine.LOCAL -> DictationSignal(localModelId ?: "other", "local", style)
        VoiceEngine.SYSTEM -> DictationSignal("android_system", "system", style)
        VoiceEngine.CLOUD -> {
            val (kind, known) = KNOWN_CLOUD_MODELS[cloud.asrProvider] ?: return DictationSignal("other", "cloud", style)
            val model = cloud.asrModel.trim()
            DictationSignal(if (model in known) "$kind:$model" else kind, "cloud", style)
        }
    }
}
