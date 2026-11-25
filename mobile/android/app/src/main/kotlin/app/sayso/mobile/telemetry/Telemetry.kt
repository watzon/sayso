package app.sayso.mobile.telemetry

import android.content.Context
import android.content.SharedPreferences
import android.os.Build
import androidx.core.content.edit
import app.sayso.mobile.BuildConfig
import java.io.IOException
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest
import java.util.UUID
import java.util.concurrent.Executor
import java.util.concurrent.Executors

/**
 * Anonymous usage signals to TelemetryDeck (Ingest API v2), the same ones the
 * desktop app sends: app.installed, app.launched, and dictation.completed
 * with the model and the kind of style. Never text, audio, prompts,
 * dictionary words, or the app typed into. The user turns it off in Setup or
 * Settings; off stops every signal at once.
 */
object Telemetry {
    /**
     * The TelemetryDeck App ID. Empty sends nothing. `-PsaysoTelemetryAppId=<id>`
     * (or the same line in gradle.properties) sets it for a build instead.
     */
    const val DEFAULT_APP_ID = ""

    const val ENDPOINT = "https://nom.telemetrydeck.com/v2/"

    const val INSTALLED = "app.installed"
    const val LAUNCHED = "app.launched"
    const val DICTATION = "dictation.completed"

    private var client: TelemetryClient? = null

    /** One client for the app process, shared by the keyboard and the companion app. */
    @Synchronized
    fun get(context: Context): TelemetryClient = client ?: TelemetryClient(
        appId = BuildConfig.TELEMETRY_APP_ID.ifBlank { DEFAULT_APP_ID },
        endpoint = ENDPOINT,
        testMode = BuildConfig.DEBUG,
        base = basePayload(BuildConfig.VERSION_NAME, if (BuildConfig.DEBUG) "dev" else "release", Build.SUPPORTED_ABIS.firstOrNull().orEmpty()),
        prefs = TelemetryPrefsStore(context),
        transport = HttpTelemetryTransport,
        executor = Executors.newSingleThreadExecutor { Thread(it, "sayso-telemetry") },
    ).also { client = it }

    /** The fields of every signal; arch uses the desktop's names (Go's GOARCH). */
    fun basePayload(appVersion: String, channel: String, abi: String): Map<String, String> = mapOf(
        "os" to "android",
        "arch" to when (abi) {
            "arm64-v8a" -> "arm64"
            "x86_64" -> "amd64"
            "armeabi-v7a" -> "arm"
            "x86" -> "386"
            else -> abi
        },
        "appVersion" to appVersion,
        "buildChannel" to channel,
        "platform" to "android",
    )
}

/** What dictation.completed tells: nothing about the words. */
data class DictationSignal(
    /** A catalog model id, "android_system", or "<provider>:<known model>" for cloud transcription. */
    val modelId: String,
    /** "local", "cloud", or "system" (Android's recognizer, on the device or not). */
    val modelKind: String,
    /** "raw" or "ai_cloud": the mobile app has no on-device styles. */
    val styleKind: String,
)

/** The install ID, whether app.installed went out, and the switch. */
interface TelemetryPrefs {
    var enabled: Boolean
    var installId: String?
    var firstLaunchSent: Boolean
}

class TelemetryPrefsStore(context: Context) : TelemetryPrefs {
    private val prefs: SharedPreferences = context.applicationContext.getSharedPreferences("telemetry", Context.MODE_PRIVATE)

    override var enabled: Boolean
        get() = prefs.getBoolean(ENABLED, true)
        set(value) = prefs.edit { putBoolean(ENABLED, value) }
    override var installId: String?
        get() = prefs.getString(INSTALL_ID, null)
        set(value) = prefs.edit { putString(INSTALL_ID, value) }
    override var firstLaunchSent: Boolean
        get() = prefs.getBoolean(FIRST_LAUNCH_SENT, false)
        set(value) = prefs.edit { putBoolean(FIRST_LAUNCH_SENT, value) }

    private companion object {
        const val ENABLED = "telemetry_enabled"
        const val INSTALL_ID = "telemetry_install_id"
        const val FIRST_LAUNCH_SENT = "telemetry_first_launch_sent"
    }
}

/** Posts a JSON body; the HTTP status, or IOException. */
fun interface TelemetryTransport {
    fun post(url: String, json: String): Int
}

object HttpTelemetryTransport : TelemetryTransport {
    override fun post(url: String, json: String): Int {
        val conn = URL(url).openConnection() as HttpURLConnection
        try {
            conn.connectTimeout = 15_000
            conn.readTimeout = 15_000
            conn.requestMethod = "POST"
            conn.doOutput = true
            conn.setRequestProperty("Content-Type", "application/json; charset=utf-8")
            conn.outputStream.use { it.write(json.toByteArray(Charsets.UTF_8)) }
            return conn.responseCode
        } finally {
            conn.disconnect()
        }
    }
}

/**
 * Queues signals and sends them on [executor]. A failed send keeps its
 * signals (at most [MAX_QUEUED]) for the next signal to take along.
 */
class TelemetryClient(
    private val appId: String,
    private val endpoint: String,
    private val testMode: Boolean,
    private val base: Map<String, String>,
    private val prefs: TelemetryPrefs,
    private val transport: TelemetryTransport,
    private val executor: Executor,
) {
    private val sessionId = UUID.randomUUID().toString()
    private val lock = Any()
    private val queue = ArrayList<String>()
    private val queuedTypes = ArrayList<String>()
    private var launched = false
    private var sending = false

    /** Bumped when telemetry is turned off, so a send already running neither retries nor marks app.installed sent. */
    private var generation = 0

    /** True when signals can leave: an App ID is set and the user has telemetry on. */
    val active: Boolean get() = appId.isNotBlank() && prefs.enabled

    var enabled: Boolean
        get() = prefs.enabled
        set(value) = synchronized(lock) {
            prefs.enabled = value
            if (!value) {
                queue.clear()
                queuedTypes.clear()
                generation++
            }
        }

    /** app.launched, after app.installed the first time; once per process. */
    fun launched() {
        synchronized(lock) {
            if (launched || !active) return
            launched = true
            if (!prefs.firstLaunchSent) enqueue(Telemetry.INSTALLED, emptyMap())
            enqueue(Telemetry.LAUNCHED, emptyMap())
        }
        schedule()
    }

    fun dictationCompleted(d: DictationSignal) {
        synchronized(lock) {
            if (!active) return
            enqueue(Telemetry.DICTATION, mapOf("modelId" to d.modelId, "modelKind" to d.modelKind, "styleKind" to d.styleKind))
        }
        schedule()
    }

    private fun enqueue(type: String, extra: Map<String, String>) {
        val id = prefs.installId ?: UUID.randomUUID().toString().also { prefs.installId = it }
        queue += signalJson(appId, clientUser(id), sessionId, type, testMode, base + extra)
        queuedTypes += type
        while (queue.size > MAX_QUEUED) {
            queue.removeAt(0)
            queuedTypes.removeAt(0)
        }
    }

    private fun schedule() = executor.execute(::flush)

    private fun flush() {
        val batch: List<String>
        val types: List<String>
        val gen: Int
        synchronized(lock) {
            if (sending || queue.isEmpty()) return
            if (!active) {
                queue.clear()
                queuedTypes.clear()
                return
            }
            batch = queue.toList()
            types = queuedTypes.toList()
            queue.clear()
            queuedTypes.clear()
            gen = generation
            sending = true
        }
        val ok = try {
            transport.post(endpoint, batch.joinToString(",", "[", "]")) / 100 == 2
        } catch (_: IOException) {
            false
        }
        synchronized(lock) {
            sending = false
            if (gen != generation || !active) return
            if (!ok) {
                queue.addAll(0, batch)
                queuedTypes.addAll(0, types)
                while (queue.size > MAX_QUEUED) {
                    queue.removeAt(0)
                    queuedTypes.removeAt(0)
                }
                return
            }
            if (Telemetry.INSTALLED in types) prefs.firstLaunchSent = true
        }
    }

    companion object {
        const val MAX_QUEUED = 50

        /** The lowercase hex SHA-256 of the install ID; TelemetryDeck hashes it again, with a salt. */
        fun clientUser(installId: String): String =
            MessageDigest.getInstance("SHA-256").digest(installId.toByteArray(Charsets.UTF_8)).joinToString("") { "%02x".format(it) }

        /** One v2 signal; the payload is flat strings only. */
        fun signalJson(appId: String, clientUser: String, sessionId: String, type: String, testMode: Boolean, payload: Map<String, String>): String =
            buildString {
                append("{\"appID\":").append(jsonString(appId))
                append(",\"clientUser\":").append(jsonString(clientUser))
                append(",\"sessionID\":").append(jsonString(sessionId))
                append(",\"type\":").append(jsonString(type))
                append(",\"isTestMode\":").append(testMode)
                append(",\"payload\":{")
                payload.entries.sortedBy { it.key }.forEachIndexed { i, (k, v) ->
                    if (i > 0) append(',')
                    append(jsonString(k)).append(':').append(jsonString(v))
                }
                append("}}")
            }

        fun jsonString(s: String): String = buildString {
            append('"')
            for (c in s) {
                when {
                    c == '"' -> append("\\\"")
                    c == '\\' -> append("\\\\")
                    c < ' ' -> append("\\u%04x".format(c.code))
                    else -> append(c)
                }
            }
            append('"')
        }
    }
}
