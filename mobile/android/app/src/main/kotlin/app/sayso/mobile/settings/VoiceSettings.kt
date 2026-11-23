package app.sayso.mobile.settings

import android.content.Context
import android.content.SharedPreferences
import androidx.core.content.edit
import app.sayso.mobile.asr.AsrModel
import app.sayso.mobile.asr.AsrModelKind
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
    /** The local model for the final text (a batch model such as Parakeet), or null. */
    val finalModelId: String? = null,
    /** The local streaming model for live text while speaking, or null. */
    val liveModelId: String? = null,
    val styleId: String = DEFAULT_STYLE,
    /** Done sends (the field's action) and starts the next dictation. */
    val handsFree: Boolean = false,
    /** Cloud transcription and Styles. Keys live in a separate private file. */
    val cloud: CloudConfig = CloudConfig.OFF,
) {
    /** Makes [model] the active model for its kind. */
    fun withActiveModel(model: AsrModel): VoiceSettings = when (model.kind) {
        AsrModelKind.OFFLINE -> copy(finalModelId = model.id)
        AsrModelKind.STREAMING -> copy(liveModelId = model.id)
    }

    /** Drops [modelId] wherever it is active, for when the model is deleted. */
    fun withoutModel(modelId: String): VoiceSettings = copy(
        finalModelId = finalModelId.takeUnless { it == modelId },
        liveModelId = liveModelId.takeUnless { it == modelId },
    )

    val cloudAsrConfigured: Boolean
        get() = cloud.asrProvider != CloudAsrProvider.OFF && cloud.asrApiKey.isNotBlank()

    companion object {
        const val DEFAULT_STYLE = "default"
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
    private const val FINAL_MODEL = "final_model"
    private const val LIVE_MODEL = "live_model"
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
        s.finalModelId?.let { put(FINAL_MODEL, it) }
        s.liveModelId?.let { put(LIVE_MODEL, it) }
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
            finalModelId = values[FINAL_MODEL]?.takeIf { it.isNotEmpty() },
            liveModelId = values[LIVE_MODEL]?.takeIf { it.isNotEmpty() },
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
