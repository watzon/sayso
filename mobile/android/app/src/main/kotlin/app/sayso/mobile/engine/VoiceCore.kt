package app.sayso.mobile.engine

/**
 * The boundary between the voice UI (keyboard voice bar, companion app) and
 * the shared Go core's voice features: text rules on a transcript, Styles,
 * opt-in cloud transcription and enhancement, and voice history.
 *
 * Local speech recognition is not here: it runs in the `:asr` process
 * (see the asr package). This interface only turns a transcript into the
 * text to insert and records what happened.
 *
 * [postProcess], [styles] and the history calls are fast and may run on the
 * main thread. [enhance] and [transcribeCloud] block on the network and must
 * run on a background thread.
 */
interface VoiceCore {
    /**
     * Applies personal dictionary words, replacements, spoken punctuation,
     * and smart spacing and capitalization for where the cursor is.
     */
    fun postProcess(raw: String, textBeforeCursor: String, textAfterCursor: String): String

    /** Built-in styles first, then custom ones. */
    fun styles(): List<VoiceStyle>

    /** Adds or replaces a custom style by id. Built-in ids are refused. */
    fun putCustomStyle(style: VoiceStyle)
    fun removeCustomStyle(id: String)

    /**
     * Rewrites [text] with the style [styleId]. Styles with no prompt, cloud
     * turned off, a timeout or an error all give back [text] unchanged with
     * [EnhanceResult.enhanced] false and the reason in [EnhanceResult.error].
     */
    fun enhance(styleId: String, text: String, cloud: CloudConfig, timeoutMs: Int = DEFAULT_TIMEOUT_MS): EnhanceResult

    /**
     * Transcribes 16 kHz mono PCM with the cloud provider in [cloud]. Null on
     * timeout, error, or when cloud transcription is off.
     */
    fun transcribeCloud(pcm16k: ShortArray, cloud: CloudConfig, timeoutMs: Int = DEFAULT_TIMEOUT_MS): String?

    /** Appends one entry. Does nothing when [sensitive] (password, privacy, incognito field). */
    fun recordHistory(entry: VoiceHistoryEntry, sensitive: Boolean)

    /** Newest first. */
    fun history(limit: Int = 200): List<VoiceHistoryEntry>
    fun deleteHistory(id: Long)
    fun clearHistory()

    companion object {
        const val DEFAULT_TIMEOUT_MS = 3000
    }
}

data class VoiceStyle(
    val id: String,
    val name: String,
    /** The instruction sent to the cloud model. Empty: text rules only, no cloud call. */
    val prompt: String,
    val builtIn: Boolean,
)

enum class CloudAsrProvider { OFF, OPENAI_WHISPER, GROQ, OPENAI_COMPATIBLE }
enum class CloudStyleProvider { OFF, OPENAI, OPENAI_COMPATIBLE, ANTHROPIC }

data class CloudConfig(
    val asrProvider: CloudAsrProvider = CloudAsrProvider.OFF,
    val asrApiKey: String = "",
    val asrBaseUrl: String = "",
    val asrModel: String = "",
    val styleProvider: CloudStyleProvider = CloudStyleProvider.OFF,
    val styleApiKey: String = "",
    val styleBaseUrl: String = "",
    val styleModel: String = "",
) {
    companion object {
        val OFF = CloudConfig()
    }
}

data class EnhanceResult(val text: String, val enhanced: Boolean, val error: String? = null)

data class VoiceHistoryEntry(
    /** Assigned by the store; 0 when recording a new entry. */
    val id: Long = 0,
    val timestampMs: Long,
    val durationMs: Long,
    val engineId: String,
    val styleId: String,
    val rawText: String,
    val finalText: String,
)

/**
 * A Kotlin stand-in for builds without the native core and for tests: text
 * rules are a trim and a space join, styles never call the network, and
 * history lives in memory.
 */
class FakeVoiceCore : VoiceCore {
    private val custom = linkedMapOf<String, VoiceStyle>()
    private val entries = mutableListOf<VoiceHistoryEntry>()
    private var nextId = 1L

    override fun postProcess(raw: String, textBeforeCursor: String, textAfterCursor: String): String {
        val t = raw.trim()
        if (t.isEmpty()) return t
        val needsSpace = textBeforeCursor.isNotEmpty() && !textBeforeCursor.last().isWhitespace()
        return if (needsSpace) " $t" else t
    }

    override fun styles(): List<VoiceStyle> = BUILT_IN + custom.values

    override fun putCustomStyle(style: VoiceStyle) {
        require(BUILT_IN.none { it.id == style.id }) { "built-in style ${style.id}" }
        custom[style.id] = style.copy(builtIn = false)
    }

    override fun removeCustomStyle(id: String) {
        custom.remove(id)
    }

    override fun enhance(styleId: String, text: String, cloud: CloudConfig, timeoutMs: Int): EnhanceResult =
        EnhanceResult(text, enhanced = false, error = if (cloud.styleProvider == CloudStyleProvider.OFF) null else "fake core")

    override fun transcribeCloud(pcm16k: ShortArray, cloud: CloudConfig, timeoutMs: Int): String? = null

    override fun recordHistory(entry: VoiceHistoryEntry, sensitive: Boolean) {
        if (sensitive) return
        entries.add(entry.copy(id = nextId++))
    }

    override fun history(limit: Int): List<VoiceHistoryEntry> = entries.asReversed().take(limit)

    override fun deleteHistory(id: Long) {
        entries.removeAll { it.id == id }
    }

    override fun clearHistory() {
        entries.clear()
    }

    private companion object {
        val BUILT_IN = listOf(VoiceStyle("default", "Default", "", builtIn = true))
    }
}
