package app.sayso.mobile.ime

import app.sayso.mobile.voice.VoiceState

/** What a tap on the dictation strip does. */
enum class VoiceStripAction { NONE, CANCEL, STOP, RETRY }

/** The middle of the dictation strip. */
sealed interface VoiceStripStatus {
    /** Recording: the waveform and the elapsed time. */
    data class Recording(val elapsed: String) : VoiceStripStatus

    /** A line of text: transcribing, a notice after an engine switch, or an error. */
    data class Message(val text: String, val tone: Tone) : VoiceStripStatus

    enum class Tone { QUIET, DANGER }
}

/**
 * The dictation strip in the place of the suggestions, above the keys, which
 * stay usable: Cancel at the left, the status in the middle, Stop (or Retry
 * after an error) at the right. Plain logic so the JVM tests cover it.
 */
object VoiceStrip {
    /** Width of each end button's touch area, in dp. */
    const val BUTTON_DP = 52f

    /** How long a notice (an engine switch) replaces the waveform. */
    const val NOTICE_MS = 3000L

    fun primary(state: VoiceState): VoiceStripAction = when (state) {
        is VoiceState.Listening -> VoiceStripAction.STOP
        is VoiceState.Failed -> VoiceStripAction.RETRY
        else -> VoiceStripAction.NONE
    }

    fun hit(x: Float, width: Float, density: Float, state: VoiceState): VoiceStripAction {
        if (state == VoiceState.Idle) return VoiceStripAction.NONE
        val button = BUTTON_DP * density
        return when {
            x < button -> VoiceStripAction.CANCEL
            x > width - button -> primary(state)
            else -> VoiceStripAction.NONE
        }
    }

    fun status(state: VoiceState, styleLabel: String, nowMs: Long): VoiceStripStatus? = when (state) {
        is VoiceState.Listening ->
            if (state.notice != null && nowMs - state.startedAtMs < NOTICE_MS) {
                VoiceStripStatus.Message(state.notice, VoiceStripStatus.Tone.QUIET)
            } else {
                VoiceStripStatus.Recording(elapsed(nowMs - state.startedAtMs))
            }
        is VoiceState.Finishing -> VoiceStripStatus.Message(
            if (state.styling && styleLabel.isNotEmpty()) "Applying $styleLabel…" else "Transcribing…",
            VoiceStripStatus.Tone.QUIET,
        )
        is VoiceState.Failed -> VoiceStripStatus.Message(state.message, VoiceStripStatus.Tone.DANGER)
        VoiceState.Idle -> null
    }

    fun elapsed(ms: Long): String {
        val secs = ms.coerceAtLeast(0) / 1000
        return "%d:%02d".format(secs / 60, secs % 60)
    }
}
