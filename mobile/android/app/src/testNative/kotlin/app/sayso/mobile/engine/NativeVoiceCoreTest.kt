package app.sayso.mobile.engine

import com.sun.net.httpserver.HttpServer
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import java.net.InetSocketAddress
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread

/**
 * NativeVoiceCore through the real JNI glue and Go core (built for this
 * computer by mobile/scripts/build-android.sh, passed in with
 * -PsaysoHostLibDir; skipped without it). Cloud calls go to a local
 * HttpServer standing in for the providers; nothing reaches a real one.
 */
class NativeVoiceCoreTest {
    @get:Rule val temp = TemporaryFolder()

    private val model = System.getProperty("sayso.model").orEmpty()
    private lateinit var journal: File
    private lateinit var typing: NativeTypingEngine
    private lateinit var voice: NativeVoiceCore
    private var server: HttpServer? = null
    private val release = CountDownLatch(1)

    @Before
    fun open() {
        assumeTrue("host JNI libraries not built (-PsaysoHostLibDir)", System.getProperty("sayso.hostLibs") == "true")
        journal = File(temp.root, "typing.journal")
        reopen()
    }

    @After
    fun close() {
        release.countDown()
        server?.stop(0)
        if (::typing.isInitialized) typing.close()
    }

    private fun reopen() {
        if (::typing.isInitialized) typing.close()
        typing = NativeTypingEngine.open(model, journal.path)
        typing.setEditorFlags(TEXT)
        voice = NativeVoiceCore(typing.handle)
    }

    /** A fake provider: chat completions and transcriptions under /ok, a hanging chat under /slow. */
    private fun startServer(): String {
        val s = HttpServer.create(InetSocketAddress("127.0.0.1", 0), 0)
        s.createContext("/ok/chat/completions") { ex ->
            val body = ex.requestBody.readBytes().decodeToString()
            val reply = if (body.contains("Talk like a pirate.")) "Arr, we were late." else "We were late."
            ex.respond(200, """{"choices":[{"message":{"content":"$reply"}}]}""")
        }
        s.createContext("/ok/audio/transcriptions") { ex ->
            ex.requestBody.readBytes()
            ex.respond(200, """{"text":"hello there"}""")
        }
        s.createContext("/slow/chat/completions") { ex ->
            ex.requestBody.readBytes()
            release.await(10, TimeUnit.SECONDS)
            ex.respond(200, """{"choices":[{"message":{"content":"too late"}}]}""")
        }
        s.executor = java.util.concurrent.Executors.newCachedThreadPool()
        s.start()
        server = s
        return "http://127.0.0.1:${s.address.port}"
    }

    private fun com.sun.net.httpserver.HttpExchange.respond(code: Int, body: String) {
        val bytes = body.encodeToByteArray()
        responseHeaders.add("Content-Type", "application/json")
        sendResponseHeaders(code, bytes.size.toLong())
        responseBody.use { it.write(bytes) }
    }

    @Test
    fun postProcessUsesTheKeyboardDictionary() {
        typing.addPersonalWord("GitHub")
        typing.putReplacement(Replacement("omw", "on my way"))
        assertEquals("push it to GitHub, please", voice.postProcess("push it to git hub comma please", "I will ", ""))
        assertEquals("On my way.", voice.postProcess("omw period", "", ""))
        assertEquals(" very", voice.postProcess("Very.", "This is", " good"))
        assertEquals("", voice.postProcess("   ", "x", ""))
    }

    @Test
    fun emojiAndAccentsSurviveTheRoundTrip() {
        assertEquals("Café 🎉 ok", voice.postProcess("café 🎉 ok", "", ""))
    }

    @Test
    fun customStylesPersistAndBuiltInsAreRefused() {
        val builtIns = voice.styles()
        assertEquals(7, builtIns.size)
        assertTrue(builtIns.all { it.builtIn })
        voice.putCustomStyle(VoiceStyle("pirate", "Pirate", "Talk like a pirate.", builtIn = false))
        var refused = false
        try {
            voice.putCustomStyle(VoiceStyle("clean", "Mine", "x", builtIn = false))
        } catch (e: IllegalArgumentException) {
            refused = true
        }
        assertTrue("a built-in id must be refused", refused)
        voice.removeCustomStyle("raw") // ignored, as in FakeVoiceCore
        reopen()
        val styles = voice.styles()
        assertEquals(8, styles.size)
        assertEquals(VoiceStyle("pirate", "Pirate", "Talk like a pirate.", builtIn = false), styles.last())
        voice.removeCustomStyle("pirate")
        assertEquals(7, voice.styles().size)
    }

    @Test
    fun historyIsNewestFirstAndSkipsPrivateFields() {
        voice.recordHistory(entry(1000, "first"), sensitive = false)
        voice.recordHistory(entry(2000, "secret"), sensitive = true)
        typing.setEditorFlags(TEXT.copy(privacy = true))
        voice.recordHistory(entry(3000, "incognito"), sensitive = false)
        typing.setEditorFlags(TEXT)
        voice.recordHistory(entry(4000, "second"), sensitive = false)
        reopen()
        val h = voice.history()
        assertEquals(listOf("second", "first"), h.map { it.finalText })
        assertEquals(VoiceHistoryEntry(1, 1000, 1500, "whisper-base", "clean", "raw first", "first"), h[1])
        assertEquals(1, voice.history(limit = 1).size)
        voice.deleteHistory(h[0].id)
        assertEquals(listOf("first"), voice.history().map { it.finalText })
        voice.clearHistory()
        assertTrue(voice.history().isEmpty())
        assertFalse(File(temp.root, "voice.journal").readText().contains("raw first"))
    }

    @Test
    fun enhanceCallsTheProviderAndFallsBack() {
        val base = startServer()
        val cloud = CloudConfig(styleProvider = CloudStyleProvider.OPENAI_COMPATIBLE, styleBaseUrl = "$base/ok", styleModel = "test")
        assertEquals(EnhanceResult("We were late.", enhanced = true), voice.enhance("polished", "we was late", cloud))
        voice.putCustomStyle(VoiceStyle("pirate", "Pirate", "Talk like a pirate.", builtIn = false))
        assertEquals(EnhanceResult("Arr, we were late.", enhanced = true), voice.enhance("pirate", "we was late", cloud))

        assertEquals(EnhanceResult("we was late", enhanced = false), voice.enhance("raw", "we was late", cloud))
        assertEquals(EnhanceResult("we was late", enhanced = false), voice.enhance("polished", "we was late", CloudConfig.OFF))
        val unknown = voice.enhance("nope", "we was late", cloud)
        assertFalse(unknown.enhanced)
        assertTrue(unknown.error!!.contains("unknown style"))

        val slow = cloud.copy(styleBaseUrl = "$base/slow")
        val started = System.nanoTime()
        val timedOut = voice.enhance("polished", "we was late", slow, timeoutMs = 300)
        assertTrue("took ${(System.nanoTime() - started) / 1_000_000} ms", System.nanoTime() - started < 3_000_000_000)
        assertEquals("we was late", timedOut.text)
        assertFalse(timedOut.enhanced)
        assertEquals("timed out", timedOut.error)
    }

    @Test
    fun typingIsNotBlockedByACloudCall() {
        val base = startServer()
        val slow = CloudConfig(styleProvider = CloudStyleProvider.OPENAI_COMPATIBLE, styleBaseUrl = "$base/slow", styleModel = "test")
        val call = thread { voice.enhance("polished", "we was late", slow, timeoutMs = 5000) }
        Thread.sleep(200)
        val started = System.nanoTime()
        typing.setContext("hello ")
        typing.suggestions()
        assertEquals("Hi", voice.postProcess("hi", "", ""))
        assertTrue("the keyboard waited on the network", System.nanoTime() - started < 500_000_000)
        release.countDown()
        call.join()
    }

    @Test
    fun transcribeCloud() {
        val base = startServer()
        val cloud = CloudConfig(asrProvider = CloudAsrProvider.OPENAI_COMPATIBLE, asrBaseUrl = "$base/ok")
        assertEquals("hello there", voice.transcribeCloud(ShortArray(16000), cloud))
        assertNull(voice.transcribeCloud(ShortArray(16000), CloudConfig.OFF))
        assertNull(voice.transcribeCloud(ShortArray(16000), cloud.copy(asrBaseUrl = "not a url")))
    }

    @Test
    fun providerOrdinalsMatchTheCore() {
        // SAYSO_ASR_* and SAYSO_STYLE_* in sayso_core.h
        assertEquals(listOf(0, 1, 2, 3), listOf(CloudAsrProvider.OFF, CloudAsrProvider.OPENAI_WHISPER, CloudAsrProvider.GROQ, CloudAsrProvider.OPENAI_COMPATIBLE).map { it.ordinal })
        assertEquals(listOf(0, 1, 2, 3), listOf(CloudStyleProvider.OFF, CloudStyleProvider.OPENAI, CloudStyleProvider.OPENAI_COMPATIBLE, CloudStyleProvider.ANTHROPIC).map { it.ordinal })
    }

    @Test
    fun callsAfterCloseFailSoftly() {
        typing.close()
        assertEquals("hi", voice.postProcess(" hi ", "", ""))
        assertTrue(voice.styles().isEmpty())
        voice.recordHistory(entry(1, "x"), sensitive = false)
        assertTrue(voice.history().isEmpty())
        typing = NativeTypingEngine.open(model, journal.path)
    }

    private fun entry(ms: Long, text: String) =
        VoiceHistoryEntry(timestampMs = ms, durationMs = 1500, engineId = "whisper-base", styleId = "clean", rawText = "raw $text", finalText = text)

    private companion object {
        val TEXT = EditorFlags(privacy = false, autoCapitalize = true, inputKind = InputKind.TEXT, suggestionsAllowed = true)
    }
}
