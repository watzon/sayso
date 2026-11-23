package app.sayso.mobile.voice

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import androidx.core.content.ContextCompat
import app.sayso.mobile.asr.ActiveModels
import app.sayso.mobile.asr.AsrClient
import app.sayso.mobile.asr.LOCAL_ASR_BUILT
import app.sayso.mobile.asr.ModelManager
import app.sayso.mobile.asr.ModelOnDisk
import app.sayso.mobile.engine.CloudConfig
import app.sayso.mobile.engine.CloudStyleProvider
import app.sayso.mobile.engine.EditorFlags
import app.sayso.mobile.engine.VoiceCore
import app.sayso.mobile.engine.VoiceHistoryEntry
import app.sayso.mobile.engine.createVoiceCore
import app.sayso.mobile.keyboard.ImeAction
import app.sayso.mobile.settings.VoiceSettings
import app.sayso.mobile.settings.VoiceSettingsStore
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors

/** One VoiceCore for the app process, shared by the keyboard and the companion app. */
object SaysoVoiceCore {
    private var core: VoiceCore? = null

    fun get(context: Context): VoiceCore =
        core ?: createVoiceCore(context.applicationContext).also { core = it }
}

/** What the controller needs from the keyboard. Called on the main thread. */
interface VoiceHost {
    fun textBeforeCursor(length: Int): String
    fun textAfterCursor(length: Int): String

    /** Inserts at the cursor through the keyboard's editor session. */
    fun insertDictation(text: String)

    /** Deletes [length] characters before the cursor. */
    fun deleteBeforeCursor(length: Int)

    /** Runs the field's action (send, search, go). */
    fun performEditorAction()

    /** Opens the companion app to ask for the microphone. */
    fun requestMicPermission()

    /** The voice state, level, or undo changed. */
    fun onVoiceChanged()
}

/**
 * Runs dictations in the keyboard: the microphone, audio focus, the three
 * engines with fallback, the text pipeline, history, undo, and hands-free.
 *
 * Engines: local sherpa-onnx in the :asr process (A), Android's
 * SpeechRecognizer (B), and cloud transcription through VoiceCore (C), in
 * the order [planEngines] gives. Main thread only, except the background
 * [worker] for network calls, whose results come back through [main].
 */
class VoiceController(context: Context, private val host: VoiceHost) {
    private val context = context.applicationContext
    private val main = Handler(Looper.getMainLooper())
    private val worker: ExecutorService = Executors.newSingleThreadExecutor { Thread(it, "sayso-voice") }
    private val settingsStore = VoiceSettingsStore(context)
    private val models = ModelManager.forApp(context)
    private val core: VoiceCore = SaysoVoiceCore.get(context)

    val machine = VoiceMachine()
    val undo = VoiceUndo()

    /** 0..1, the loudness of the latest audio, for the waveform. */
    var level = 0f
        private set
    var handsFree = false
        private set

    private var settings = VoiceSettings()
    private var policy = VoicePolicy(sensitive = false)
    private var imeAction = ImeAction.NEWLINE
    private var plan: List<VoiceEngine> = emptyList()
    private var active = ActiveModels(null, null)
    private var streamingToAsr = false
    private var sendAfterInsert = false
    private val buffer = PcmBuffer(maxSamples = (VoiceMachine.MAX_RECORDING_MS / 1000 * Pcm.SAMPLE_RATE).toInt())

    /** After the :asr process dies, the local engine is skipped until this time. */
    private var localBrokenUntilMs = 0L

    private val focus = DictationAudioFocus(context) { done() }
    private val capture = AudioCapture(
        onChunk = { pcm, lvl -> main.post { onChunk(pcm, lvl) } },
        onError = { main.post { (machine.state as? VoiceState.Listening)?.let { fail(it.session, "The microphone stopped") } } },
    )
    private var system: SystemRecognizer? = null

    private val asr = AsrClient(context, object : AsrClient.Listener {
        override fun onAsrPartial(session: Int, text: String) {
            if (machine.partial(session, text)) host.onVoiceChanged()
        }

        override fun onAsrFinal(session: Int, text: String) {
            main.removeCallbacks(finalTimeout)
            onTranscript(session, text)
        }

        override fun onAsrError(session: Int, message: String, processDied: Boolean) = onLocalFailed(session, message)
    })

    private val finalTimeout = Runnable {
        val s = machine.state as? VoiceState.Finishing ?: return@Runnable
        if (!s.styling) fail(s.session, "The on-device engine took too long")
    }

    val engineLabel: String
        get() = when (val s = machine.state) {
            is VoiceState.Listening -> s.engine.label
            is VoiceState.Finishing -> s.engine.label
            else -> ""
        }

    val styleLabel: String
        get() = core.styles().firstOrNull { it.id == settings.styleId }?.name ?: "Default"

    /** Done sends: hands-free is on and the field has an action. */
    val doneSends: Boolean get() = handsFree && handsFreeSends(imeAction)

    private fun now() = SystemClock.elapsedRealtime()

    /** The mic button: starts a dictation, or finishes the one that is listening. */
    fun micTapped(flags: EditorFlags, action: ImeAction) {
        when (machine.state) {
            is VoiceState.Listening -> return done()
            is VoiceState.Finishing -> return
            else -> {}
        }
        if (ContextCompat.checkSelfPermission(context, Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
            host.requestMicPermission()
            return
        }
        settings = settingsStore.load()
        handsFree = settings.handsFree
        policy = VoicePolicy.forEditor(flags)
        imeAction = action
        machine.cancel()
        begin()
    }

    private fun begin() {
        undo.clear()
        // Only Done sends; a dictation the system recognizer ends by itself does not.
        sendAfterInsert = false
        active = models.activeModels()
        val available = EngineAvailability(
            local = LOCAL_ASR_BUILT && active.any && now() >= localBrokenUntilMs,
            system = SystemRecognizer.available(context),
            cloud = settings.cloudAsrConfigured,
        )
        plan = planEngines(settings.engine, available, policy)
        val engine = plan.firstOrNull()
        if (engine == null) {
            machine.refuse("No speech recognizer on this device. Download a model in the Sayso app.")
            host.onVoiceChanged()
            return
        }
        val session = machine.start(engine, now()) ?: return
        focus.request()
        startEngine(session, engine)
        host.onVoiceChanged()
    }

    private fun startEngine(session: Int, engine: VoiceEngine) {
        level = 0f
        buffer.clear()
        streamingToAsr = false
        when (engine) {
            VoiceEngine.LOCAL, VoiceEngine.CLOUD -> {
                if (!capture.start()) return onMicUnavailable(session, engine)
                // Cloud also streams to the local engine when there is one: live text, and the fallback.
                if (engine == VoiceEngine.LOCAL || (VoiceEngine.LOCAL in plan)) {
                    streamingToAsr = true
                    asr.start(session, active.final?.let(::onDisk), active.live?.let(::onDisk))
                }
            }
            VoiceEngine.SYSTEM -> startSystem(session)
        }
    }

    private fun onDisk(m: app.sayso.mobile.asr.AsrModel) = ModelOnDisk(m, models.modelDir(m))

    private fun startSystem(session: Int) {
        system?.cancel()
        system = SystemRecognizer(
            context,
            onLevel = { if (isCurrent(session)) setLevel(it) },
            onPartial = { if (machine.partial(session, it)) host.onVoiceChanged() },
            onFinal = { focus.abandon(); onTranscript(session, it) },
            onError = { fail(session, it) },
        ).also { it.start(policy.sensitive) }
    }

    private fun isCurrent(session: Int) = (machine.state as? VoiceState.Listening)?.session == session

    private fun setLevel(value: Float) {
        level = value
        host.onVoiceChanged()
    }

    private fun onChunk(pcm: ShortArray, lvl: Float) {
        val s = machine.state as? VoiceState.Listening ?: return
        if (s.engine == VoiceEngine.SYSTEM) return
        setLevel(lvl)
        val fits = buffer.append(pcm)
        if (streamingToAsr) asr.audio(s.session, pcm)
        if (!fits || machine.shouldAutoStop(now())) done()
    }

    /** The microphone did not open: let the system recognizer try, it opens the microphone its own way. */
    private fun onMicUnavailable(session: Int, failed: VoiceEngine) {
        if (VoiceEngine.SYSTEM in plan && failed != VoiceEngine.SYSTEM) {
            val next = machine.switchEngine(session, VoiceEngine.SYSTEM, now(), "Using the system recognizer") ?: return
            startSystem(next)
        } else {
            fail(session, "The microphone is in use")
        }
    }

    /** Done (✓): stop recording and get the final text. */
    fun done() {
        val s = machine.state as? VoiceState.Listening ?: return
        if (!machine.stop(s.session, now())) return
        sendAfterInsert = doneSends
        focus.abandon()
        when (s.engine) {
            VoiceEngine.LOCAL -> {
                capture.stop()
                asr.finish(s.session)
                main.postDelayed(finalTimeout, FINAL_TIMEOUT_MS + Pcm.durationMs(buffer.size))
            }
            VoiceEngine.CLOUD -> {
                capture.stop()
                transcribeInCloud(s.session, buffer.toShortArray(), policy.cloudConfig(settings))
            }
            VoiceEngine.SYSTEM -> system?.stop()
        }
        host.onVoiceChanged()
    }

    private fun transcribeInCloud(session: Int, pcm: ShortArray, cloud: CloudConfig) {
        worker.execute {
            val text = runCatching { core.transcribeCloud(pcm, cloud) }.getOrNull()
            main.post {
                when {
                    !text.isNullOrBlank() -> {
                        if (streamingToAsr) asr.cancel(session)
                        onTranscript(session, text)
                    }
                    streamingToAsr -> {
                        // The local engine heard the same audio: let it give the text.
                        asr.finish(session)
                        main.postDelayed(finalTimeout, FINAL_TIMEOUT_MS + Pcm.durationMs(pcm.size))
                    }
                    else -> fail(session, "Cloud transcription failed")
                }
            }
        }
    }

    private fun onLocalFailed(session: Int, message: String) {
        localBrokenUntilMs = now() + LOCAL_RETRY_MS
        streamingToAsr = false
        when (val s = machine.state) {
            is VoiceState.Listening -> {
                if (s.session != session) return
                if (s.engine == VoiceEngine.CLOUD) return // the local engine only gave live text
                val next = fallbackAfter(VoiceEngine.LOCAL, plan)
                if (next == VoiceEngine.SYSTEM) {
                    // SpeechRecognizer records the microphone itself: release ours first.
                    capture.stop()
                    val n = machine.switchEngine(session, VoiceEngine.SYSTEM, now(), "$message. Using the system recognizer.") ?: return
                    startSystem(n)
                    host.onVoiceChanged()
                } else {
                    fail(session, message)
                }
            }
            is VoiceState.Finishing -> if (s.session == session) fail(session, "$message. Try again.")
            else -> {}
        }
    }

    /** The raw transcript is in: post-process, style, insert. */
    private fun onTranscript(session: Int, raw: String) {
        if (raw.isBlank()) {
            fail(session, "Didn't catch that")
            return
        }
        val state = machine.state
        val startedAt = when (state) {
            is VoiceState.Listening -> state.startedAtMs
            is VoiceState.Finishing -> state.startedAtMs
            else -> return
        }
        val engine = (state as? VoiceState.Listening)?.engine ?: (state as VoiceState.Finishing).engine
        if (!machine.transcribed(session, now())) return
        val stoppedAt = (machine.state as VoiceState.Finishing).stoppedAtMs
        val text = core.postProcess(raw, host.textBeforeCursor(CONTEXT), host.textAfterCursor(CONTEXT))
        val record = VoiceHistoryEntry(
            timestampMs = System.currentTimeMillis(),
            durationMs = stoppedAt - startedAt,
            engineId = engine.id,
            styleId = settings.styleId,
            rawText = raw,
            finalText = text,
        )
        val style = core.styles().firstOrNull { it.id == settings.styleId }
        val cloud = policy.cloudConfig(settings)
        host.onVoiceChanged()
        if (style == null || style.prompt.isBlank() || cloud.styleProvider == CloudStyleProvider.OFF) {
            insert(session, text, record)
            return
        }
        worker.execute {
            // enhance gives the text back unchanged on timeout or error.
            val styled = runCatching { core.enhance(style.id, text, cloud).text }.getOrDefault(text)
            main.post { insert(session, styled, record.copy(finalText = styled)) }
        }
    }

    private fun insert(session: Int, text: String, record: VoiceHistoryEntry) {
        if (!machine.complete(session)) return
        asr.idle()
        if (text.isNotEmpty()) {
            host.insertDictation(text)
            undo.record(text)
            core.recordHistory(record, sensitive = policy.sensitive)
        }
        if (handsFree) {
            if (sendAfterInsert) {
                host.performEditorAction()
                undo.clear()
            }
            begin() // talk, send, talk
        } else {
            host.onVoiceChanged()
        }
    }

    private fun fail(session: Int, message: String) {
        if (!machine.fail(session, message)) return
        stopEverything()
        host.onVoiceChanged()
    }

    /** Cancel (×), or close the bar after an error. */
    fun cancel() {
        if (machine.state == VoiceState.Idle) return
        machine.cancel()
        stopEverything()
        host.onVoiceChanged()
    }

    fun toggleHandsFree() {
        handsFree = !handsFree
        settingsStore.update { it.copy(handsFree = handsFree) }
        host.onVoiceChanged()
    }

    /** The one-tap undo chip. True when it removed the dictated text. */
    fun undoLast(): Boolean {
        val text = undo.inserted ?: return false
        val action = undo.undo(host.textBeforeCursor(text.length)) ?: return false
        host.deleteBeforeCursor(action.deleteBefore)
        return true
    }

    private fun stopEverything() {
        main.removeCallbacks(finalTimeout)
        capture.stop()
        system?.cancel()
        system = null
        focus.abandon()
        asr.cancelCurrent()
        streamingToAsr = false
        level = 0f
    }

    fun destroy() {
        cancel()
        asr.release()
        worker.shutdownNow()
    }

    private companion object {
        const val CONTEXT = 200
        const val FINAL_TIMEOUT_MS = 20_000L
        const val LOCAL_RETRY_MS = 5 * 60_000L
    }
}
