package app.sayso.mobile.telemetry

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.IOException
import java.security.MessageDigest

private class MemoryPrefs : TelemetryPrefs {
    override var enabled = true
    override var installId: String? = null
    override var firstLaunchSent = false
}

/** Keeps each body; answers [status], or throws when [down]. */
private class FakeTransport : TelemetryTransport {
    val bodies = mutableListOf<String>()
    val urls = mutableListOf<String>()
    var status = 200
    var down = false
    var during: (() -> Unit)? = null

    override fun post(url: String, json: String): Int {
        urls += url
        bodies += json
        during?.invoke()
        if (down) throw IOException("offline")
        return status
    }

    fun types(): List<String> = bodies.flatMap { TYPE.findAll(it).map { m -> m.groupValues[1] }.toList() }

    companion object {
        val TYPE = Regex("\"type\":\"([^\"]+)\"")
    }
}

class TelemetryClientTest {
    private val prefs = MemoryPrefs()
    private val transport = FakeTransport()
    private val base = Telemetry.basePayload("0.1.0", "dev", "arm64-v8a")

    private fun client(appId: String = "APP-ID", testMode: Boolean = true) =
        TelemetryClient(appId, "https://example.test/v2/", testMode, base, prefs, transport) { it.run() }

    @Test
    fun signalsFollowTheV2Schema() {
        val c = client()
        c.launched()
        c.dictationCompleted(DictationSignal("whisper-base-en", "local", "ai_cloud"))
        assertEquals(listOf("app.installed", "app.launched", "dictation.completed"), transport.types())
        val id = prefs.installId!!
        assertTrue(Regex("^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$").matches(id))
        val digest = MessageDigest.getInstance("SHA-256").digest(id.toByteArray()).joinToString("") { "%02x".format(it) }
        assertEquals(digest, TelemetryClient.clientUser(id))
        assertEquals(listOf("https://example.test/v2/"), transport.urls.distinct())

        val body = transport.bodies.last()
        assertTrue(body, body.startsWith("[{") && body.endsWith("}]"))
        val signal = Regex("\\{\"appID\":\"APP-ID\",\"clientUser\":\"([0-9a-f]{64})\",\"sessionID\":\"([0-9a-f-]{36})\",\"type\":\"dictation.completed\",\"isTestMode\":true,\"payload\":\\{([^{}]*)\\}\\}")
            .find(body) ?: error("no dictation signal in $body")
        assertEquals(digest, signal.groupValues[1])
        assertNotEquals(id, signal.groupValues[1])
        assertEquals(
            "\"appVersion\":\"0.1.0\",\"arch\":\"arm64\",\"buildChannel\":\"dev\",\"modelId\":\"whisper-base-en\",\"modelKind\":\"local\"," +
                "\"os\":\"android\",\"platform\":\"android\",\"styleKind\":\"ai_cloud\"",
            signal.groupValues[3],
        )
    }

    @Test
    fun installedIsSentOncePerInstall() {
        val c = client()
        c.launched()
        c.launched()
        assertEquals(listOf("app.installed", "app.launched"), transport.types())
        assertTrue(prefs.firstLaunchSent)
        val user = TelemetryClient.clientUser(prefs.installId!!)

        transport.bodies.clear()
        client().launched() // the next process
        assertEquals(listOf("app.launched"), transport.types())
        assertTrue(transport.bodies.single().contains(user))
    }

    @Test
    fun aFailedSendKeepsInstalledForTheNextSignal() {
        transport.down = true
        val c = client()
        c.launched()
        assertFalse(prefs.firstLaunchSent)
        transport.down = false
        transport.bodies.clear()
        c.dictationCompleted(DictationSignal("m", "local", "raw"))
        assertEquals(listOf("app.installed", "app.launched", "dictation.completed"), transport.types())
        assertTrue(prefs.firstLaunchSent)

        transport.status = 503
        transport.bodies.clear()
        val again = MemoryPrefs()
        TelemetryClient("APP-ID", "https://example.test/v2/", true, base, again, transport) { it.run() }.launched()
        assertFalse("a refused app.installed was marked sent", again.firstLaunchSent)
    }

    @Test
    fun offSendsNothingAndStoresNoInstallId() {
        prefs.enabled = false
        val c = client()
        c.launched()
        c.dictationCompleted(DictationSignal("m", "local", "raw"))
        assertTrue(transport.bodies.isEmpty())
        assertEquals(null, prefs.installId)
    }

    @Test
    fun turningOffDropsQueuedSignalsAndTheRetry() {
        transport.down = true
        val c = client()
        c.launched()
        c.enabled = false
        c.enabled = true
        transport.down = false
        transport.bodies.clear()
        c.dictationCompleted(DictationSignal("m", "local", "raw"))
        assertEquals("signals from before off were sent", listOf("dictation.completed"), transport.types())
    }

    @Test
    fun turningOffDuringASendKeepsItFromBeingRetried() {
        transport.down = true
        val c = client()
        transport.during = { c.enabled = false }
        c.launched()
        transport.during = null
        c.enabled = true
        transport.down = false
        transport.bodies.clear()
        c.dictationCompleted(DictationSignal("m", "local", "raw"))
        assertEquals(listOf("dictation.completed"), transport.types())
        assertFalse(prefs.firstLaunchSent)
    }

    @Test
    fun emptyAppIdSendsNothing() {
        val c = client(appId = " ")
        assertFalse(c.active)
        c.launched()
        c.dictationCompleted(DictationSignal("m", "local", "raw"))
        assertTrue(transport.bodies.isEmpty())
        assertEquals("", Telemetry.DEFAULT_APP_ID)
    }

    @Test
    fun releaseBuildsAreNotInTestMode() {
        client(testMode = false).launched()
        assertTrue(transport.bodies.single().contains("\"isTestMode\":false"))
        assertFalse(transport.bodies.single().contains("\"isTestMode\":true"))
    }

    @Test
    fun jsonStringsAreEscaped() {
        assertEquals("\"a\\\"b\\\\c\\u000a\"", TelemetryClient.jsonString("a\"b\\c\n"))
    }

    @Test
    fun archUsesTheDesktopNames() {
        assertEquals("amd64", Telemetry.basePayload("1", "release", "x86_64")["arch"])
        assertEquals("arm64", Telemetry.basePayload("1", "release", "arm64-v8a")["arch"])
    }
}
