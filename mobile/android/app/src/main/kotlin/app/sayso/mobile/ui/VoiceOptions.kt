package app.sayso.mobile.ui

import app.sayso.mobile.asr.AsrModel
import app.sayso.mobile.asr.ModelState
import app.sayso.mobile.asr.RamAdvisory
import app.sayso.mobile.engine.CloudAsrProvider
import app.sayso.mobile.engine.CloudStyleProvider
import app.sayso.mobile.engine.VoiceStyle
import java.util.Locale

/** A model download the app is running, or the last one that failed. */
sealed interface DownloadStatus {
    data class Running(val downloadedBytes: Long, val totalBytes: Long) : DownloadStatus
    data class Failed(val message: String) : DownloadStatus
}

enum class ModelBadge(val label: String) {
    NOT_DOWNLOADED("Not downloaded"),
    PAUSED("Paused"),
    DOWNLOADING("Downloading"),
    FAILED("Failed"),
    DOWNLOADED("Downloaded"),
    ACTIVE("Active"),
}

/** The badge of one model: a running or failed download first, then what is on disk. */
fun modelBadge(state: ModelState, download: DownloadStatus?, active: Boolean): ModelBadge = when {
    download is DownloadStatus.Running -> ModelBadge.DOWNLOADING
    state == ModelState.Installed -> if (active) ModelBadge.ACTIVE else ModelBadge.DOWNLOADED
    download is DownloadStatus.Failed -> ModelBadge.FAILED
    state is ModelState.Partial -> ModelBadge.PAUSED
    else -> ModelBadge.NOT_DOWNLOADED
}

/** Whole percent, never 100 before the last byte. */
fun downloadPercent(downloaded: Long, total: Long): Int {
    if (total <= 0) return 0
    if (downloaded >= total) return 100
    return (downloaded.coerceAtLeast(0) * 100 / total).toInt().coerceAtMost(99)
}

/** Download sizes in decimal units, as stores and the README show them. */
fun formatBytes(bytes: Long): String = when {
    bytes >= 1_000_000_000 -> String.format(Locale.US, "%.2f GB", bytes / 1e9)
    bytes >= 1_000_000 -> String.format(Locale.US, "%.1f MB", bytes / 1e6)
    else -> String.format(Locale.US, "%d KB", (bytes + 999) / 1000)
}

/** A model's size at a glance, in whole decimal megabytes. */
fun formatSize(bytes: Long): String =
    if (bytes >= 1_000_000_000) String.format(Locale.US, "%.1f GB", bytes / 1e9)
    else "${Math.round(bytes / 1e6)} MB"

/** Device RAM in binary GB, the way phones advertise it. */
fun formatRam(bytes: Long): String = String.format(Locale.US, "%.1f GB", bytes / (1L shl 30).toDouble())

/** One line on the languages the model knows. */
fun modelLanguages(model: AsrModel): String =
    if (model.languages.size == 1) Locale.forLanguageTag(model.languages[0]).getDisplayLanguage(Locale.US)
    else "${model.languages.size} languages"

/** What a model card says about memory; [warning] when this phone has too little. */
data class RamNote(val text: String, val warning: Boolean)

/** Nothing for a model with no memory need, a plain note when the phone has enough, else a warning. */
fun ramNote(model: AsrModel, advisory: RamAdvisory?): RamNote? = when {
    model.recommendedRamBytes <= 0 -> null
    advisory != null -> RamNote(
        "This phone has ${formatRam(advisory.deviceRamBytes)} of memory; this model wants ${formatRam(model.recommendedRamBytes)}. It may be slow or stop.",
        warning = true,
    )
    else -> RamNote("Wants ${formatRam(model.recommendedRamBytes)} of memory.", warning = false)
}

/** The line under a model's name: what it is good at, then its languages. */
fun modelLine(model: AsrModel): String = "${model.summary} · ${modelLanguages(model)}"

/** The style the keyboard will use: [id] when it exists, else the first, as the keyboard resolves it. */
fun effectiveStyleId(styles: List<VoiceStyle>, id: String): String? =
    styles.firstOrNull { it.id == id }?.id ?: styles.firstOrNull()?.id

fun asrProviderName(p: CloudAsrProvider) = when (p) {
    CloudAsrProvider.OFF -> "Off"
    CloudAsrProvider.OPENAI_WHISPER -> "OpenAI Whisper"
    CloudAsrProvider.GROQ -> "Groq"
    CloudAsrProvider.OPENAI_COMPATIBLE -> "OpenAI-compatible server"
}

fun styleProviderName(p: CloudStyleProvider) = when (p) {
    CloudStyleProvider.OFF -> "Off"
    CloudStyleProvider.OPENAI -> "OpenAI"
    CloudStyleProvider.OPENAI_COMPATIBLE -> "OpenAI-compatible server"
    CloudStyleProvider.ANTHROPIC -> "Anthropic"
}

/**
 * What the core uses when a field is left empty (mobile/core/cloud), shown
 * as the field's placeholder. A compatible server has no default: it must
 * be filled in.
 */
data class CloudFieldHints(val baseUrl: String, val model: String, val needsBaseUrl: Boolean, val needsModel: Boolean)

fun asrHints(p: CloudAsrProvider): CloudFieldHints = when (p) {
    CloudAsrProvider.OPENAI_WHISPER -> CloudFieldHints("https://api.openai.com/v1", "whisper-1", needsBaseUrl = false, needsModel = false)
    CloudAsrProvider.GROQ -> CloudFieldHints("https://api.groq.com/openai/v1", "whisper-large-v3-turbo", needsBaseUrl = false, needsModel = false)
    CloudAsrProvider.OPENAI_COMPATIBLE, CloudAsrProvider.OFF ->
        CloudFieldHints("https://your-server/v1", "Model name", needsBaseUrl = true, needsModel = true)
}

fun styleHints(p: CloudStyleProvider): CloudFieldHints = when (p) {
    CloudStyleProvider.OPENAI -> CloudFieldHints("https://api.openai.com/v1", "gpt-5-mini", needsBaseUrl = false, needsModel = false)
    CloudStyleProvider.ANTHROPIC -> CloudFieldHints("https://api.anthropic.com/v1", "claude-haiku-4-5", needsBaseUrl = false, needsModel = false)
    CloudStyleProvider.OPENAI_COMPATIBLE, CloudStyleProvider.OFF ->
        CloudFieldHints("https://your-server/v1", "Model name", needsBaseUrl = true, needsModel = true)
}
