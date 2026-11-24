package app.sayso.mobile.settings

import android.content.Context
import android.content.SharedPreferences
import androidx.core.content.edit
import app.sayso.mobile.asr.AsrModel
import app.sayso.mobile.engine.CloudAsrProvider
import app.sayso.mobile.engine.CloudConfig
import app.sayso.mobile.engine.CloudStyleProvider

/**
 * Which recognizer a dictation starts with. Every choice falls back to the
 * system recognizer when its own engine cannot run.
 */
enum class VoiceEngineChoice {
    /** The local model when one is active, else the system recognizer. */
    AUTO,

    /** Android's SpeechRecognizer, even when a local model is active. */
    SYSTEM,

    /** The cloud provider in [VoiceSettings.cloud]; never in private fields. */
    CLOUD,
}

data class VoiceSettings(
    val engine: VoiceEngineChoice = VoiceEngineChoice.AUTO,
    /** The local model that transcribes a dictation, or null. */
    val modelId: String? = null,
    val styleId: String = DEFAULT_STYLE,
    /** Done sends (the field's action) and starts the next dictation. */
    val handsFree: Boolean = false,
    /** Cloud transcription and Styles. Keys live in a separate private file. */
    val cloud: CloudConfig = CloudConfig.OFF,
) {
    fun withActiveModel(model: AsrModel): VoiceSettings = copy(modelId = model.id)

    /** Drops [id] when it is the active model, for when the model is deleted. */
    fun withoutModel(id: String): VoiceSettings = copy(modelId = modelId.takeUnless { it == id })

    val cloudAsrConfigured: Boolean
        get() = when (cloud.asrProvider) {
            CloudAsrProvider.OFF -> false
            // A self-hosted server may need no key, but the core needs its URL and model.
            CloudAsrProvider.OPENAI_COMPATIBLE -> cloud.asrBaseUrl.isNotBlank() && cloud.asrModel.isNotBlank()
            else -> cloud.asrApiKey.isNotBlank()
        }

    companion object {
        /** The desktop's first built-in style. The keyboard uses the first of VoiceCore.styles() when an id is unknown. */
        const val DEFAULT_STYLE = "clean"
    }
}

/** Where [app.sayso.mobile.asr.ModelManager] reads and writes the active models. */
interface VoiceSettingsRepository {
    fun load(): VoiceSettings
    fun save(settings: VoiceSettings)
}

/**
 * VoiceSettings to and from flat string maps. API keys go to their own map,
 * which the store keeps in a separate private file.
 */
object VoiceSettingsCodec {
    private const val ENGINE = "engine"
    private const val MODEL = "final_model"
    private const val STYLE = "style"
    private const val HANDS_FREE = "hands_free"
    private const val ASR_PROVIDER = "cloud_asr_provider"
    private const val ASR_BASE_URL = "cloud_asr_base_url"
    private const val ASR_MODEL = "cloud_asr_model"
    private const val STYLE_PROVIDER = "cloud_style_provider"
    private const val STYLE_BASE_URL = "cloud_style_base_url"
    private const val STYLE_MODEL = "cloud_style_model"
    private const val ASR_KEY = "cloud_asr_api_key"
    private const val STYLE_KEY = "cloud_style_api_key"

    fun encode(s: VoiceSettings): Map<String, String> = buildMap {
        put(ENGINE, s.engine.name)
        s.modelId?.let { put(MODEL, it) }
        put(STYLE, s.styleId)
        put(HANDS_FREE, s.handsFree.toString())
        put(ASR_PROVIDER, s.cloud.asrProvider.name)
        put(ASR_BASE_URL, s.cloud.asrBaseUrl)
        put(ASR_MODEL, s.cloud.asrModel)
        put(STYLE_PROVIDER, s.cloud.styleProvider.name)
        put(STYLE_BASE_URL, s.cloud.styleBaseUrl)
        put(STYLE_MODEL, s.cloud.styleModel)
    }

    fun encodeSecrets(s: VoiceSettings): Map<String, String> = mapOf(
        ASR_KEY to s.cloud.asrApiKey,
        STYLE_KEY to s.cloud.styleApiKey,
    )

    fun decode(values: Map<String, String>, secrets: Map<String, String>): VoiceSettings {
        val d = VoiceSettings()
        return VoiceSettings(
            engine = enumOr(values[ENGINE], d.engine),
            modelId = values[MODEL]?.takeIf { it.isNotEmpty() },
            styleId = values[STYLE]?.takeIf { it.isNotEmpty() } ?: d.styleId,
            handsFree = values[HANDS_FREE]?.toBooleanStrictOrNull() ?: d.handsFree,
            cloud = CloudConfig(
                asrProvider = enumOr(values[ASR_PROVIDER], CloudAsrProvider.OFF),
                asrApiKey = secrets[ASR_KEY].orEmpty(),
                asrBaseUrl = values[ASR_BASE_URL].orEmpty(),
                asrModel = values[ASR_MODEL].orEmpty(),
                styleProvider = enumOr(values[STYLE_PROVIDER], CloudStyleProvider.OFF),
                styleApiKey = secrets[STYLE_KEY].orEmpty(),
                styleBaseUrl = values[STYLE_BASE_URL].orEmpty(),
                styleModel = values[STYLE_MODEL].orEmpty(),
            ),
        )
    }

    private inline fun <reified E : Enum<E>> enumOr(name: String?, default: E): E =
        enumValues<E>().firstOrNull { it.name == name } ?: default
}

/**
 * Voice settings in app-private SharedPreferences, written by the companion
 * app and read by the keyboard when a dictation starts. API keys are in a
 * second file so a settings export or log of the first never holds them;
 * the app sets allowBackup="false", so neither leaves the device.
 */
class VoiceSettingsStore(context: Context) : VoiceSettingsRepository {
    private val app = context.applicationContext
    private val prefs: SharedPreferences = app.getSharedPreferences("voice", Context.MODE_PRIVATE)
    private val secrets: SharedPreferences = app.getSharedPreferences("voice_secrets", Context.MODE_PRIVATE)

    override fun load(): VoiceSettings = VoiceSettingsCodec.decode(strings(prefs), strings(secrets))

    override fun save(settings: VoiceSettings) {
        prefs.edit {
            clear()
            VoiceSettingsCodec.encode(settings).forEach { (k, v) -> putString(k, v) }
        }
        secrets.edit {
            clear()
            VoiceSettingsCodec.encodeSecrets(settings).forEach { (k, v) -> putString(k, v) }
        }
    }

    fun update(change: (VoiceSettings) -> VoiceSettings) = save(change(load()))

    private fun strings(p: SharedPreferences): Map<String, String> =
        p.all.mapNotNull { (k, v) -> (v as? String)?.let { k to it } }.toMap()
}
