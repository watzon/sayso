package app.sayso.mobile.voice

/** The recognizers a dictation can use. [id] is what voice history records. */
enum class VoiceEngine(val id: String, val label: String) {
    /** sherpa-onnx in the :asr process, with a downloaded model. */
    LOCAL("local", "On-device"),

    /** Android's SpeechRecognizer. Records the microphone itself. */
    SYSTEM("system", "System"),

    /** VoiceCore.transcribeCloud on the recorded audio. */
    CLOUD("cloud", "Cloud"),
}

sealed interface VoiceState {
    data object Idle : VoiceState

    data class Listening(
        val session: Int,
        val engine: VoiceEngine,
        val startedAtMs: Long,
        val partial: String = "",
        /** Shown in the bar, for example after a switch to another engine. */
        val notice: String? = null,
    ) : VoiceState

    /** Recording stopped; the final text is on its way ([styling] false) or being post-processed and styled. */
    data class Finishing(
        val session: Int,
        val engine: VoiceEngine,
        val startedAtMs: Long,
        val stoppedAtMs: Long,
        val partial: String,
        val styling: Boolean = false,
    ) : VoiceState

    /** Nothing was inserted. The bar shows [message] until the user closes it or tries again. */
    data class Failed(val message: String) : VoiceState
}

/**
 * The life of one dictation: listen, stop, transcribe, style, insert.
 *
 * Each start gets a new session number. Recognizers, the :asr process and
 * background work report back with the session they were started for, and
 * the machine drops anything from an older session. That is what keeps a
 * late partial, a final after Cancel, or a style result after the user
 * started over from reaching the editor.
 */
class VoiceMachine {
    var state: VoiceState = VoiceState.Idle
        private set

    private var lastSession = 0

    val active: Boolean get() = state is VoiceState.Listening || state is VoiceState.Finishing

    /** Starts a dictation. Refused (null) while one is listening or finishing. */
    fun start(engine: VoiceEngine, nowMs: Long): Int? {
        if (active) return null
        val session = ++lastSession
        state = VoiceState.Listening(session, engine, nowMs)
        return session
    }

    fun partial(session: Int, text: String): Boolean {
        val s = state as? VoiceState.Listening ?: return false
        if (s.session != session) return false
        state = s.copy(partial = text)
        return true
    }

    /** The user tapped Done, or the recording hit [MAX_RECORDING_MS]. */
    fun stop(session: Int, nowMs: Long): Boolean {
        val s = state as? VoiceState.Listening ?: return false
        if (s.session != session) return false
        state = VoiceState.Finishing(s.session, s.engine, s.startedAtMs, nowMs, s.partial)
        return true
    }

    /**
     * The final transcript arrived. Also accepted while listening, because the
     * system recognizer ends by itself after a pause. False for a stale session.
     */
    fun transcribed(session: Int, nowMs: Long): Boolean {
        when (val s = state) {
            is VoiceState.Listening -> if (s.session != session) return false else {
                state = VoiceState.Finishing(s.session, s.engine, s.startedAtMs, nowMs, s.partial, styling = true)
            }
            is VoiceState.Finishing -> if (s.session != session || s.styling) return false else {
                state = s.copy(styling = true)
            }
            else -> return false
        }
        return true
    }

    /** The text is ready to insert. True means insert it now; false means the dictation was cancelled or replaced. */
    fun complete(session: Int): Boolean {
        val s = state as? VoiceState.Finishing ?: return false
        if (s.session != session || !s.styling) return false
        state = VoiceState.Idle
        return true
    }

    /**
     * The engine of [session] stopped working while listening, and [engine]
     * takes over. Starts a new session (so the old engine's callbacks are
     * dropped) and keeps the bar open with [notice].
     */
    fun switchEngine(session: Int, engine: VoiceEngine, nowMs: Long, notice: String): Int? {
        val s = state as? VoiceState.Listening ?: return null
        if (s.session != session) return null
        val next = ++lastSession
        state = VoiceState.Listening(next, engine, nowMs, notice = notice)
        return next
    }

    fun fail(session: Int, message: String): Boolean {
        val current = when (val s = state) {
            is VoiceState.Listening -> s.session
            is VoiceState.Finishing -> s.session
            else -> return false
        }
        if (current != session) return false
        state = VoiceState.Failed(message)
        return true
    }

    /** No engine can start: shows [message] instead of listening. Ignored while a dictation runs. */
    fun refuse(message: String): Boolean {
        if (active) return false
        state = VoiceState.Failed(message)
        return true
    }

    /** Cancel, or close the bar after a failure. Everything still running for the old session is ignored from now on. */
    fun cancel() {
        state = VoiceState.Idle
    }

    fun shouldAutoStop(nowMs: Long): Boolean {
        val s = state as? VoiceState.Listening ?: return false
        return nowMs - s.startedAtMs >= MAX_RECORDING_MS
    }

    companion object {
        /** A longer recording stops by itself: the buffer and the batch model hold the whole clip in memory. */
        const val MAX_RECORDING_MS = 5 * 60 * 1000L
    }
}
