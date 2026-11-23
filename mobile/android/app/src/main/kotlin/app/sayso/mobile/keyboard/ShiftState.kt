package app.sayso.mobile.keyboard

enum class Shift { OFF, ONCE, AUTO, LOCKED }

/**
 * Shift and Caps Lock. A tap turns shift on for one letter; a second tap
 * within [doubleTapMs] locks it; a tap while on turns it off. AUTO is the
 * one-letter shift that auto-capitalization sets; it follows the editor's
 * caps mode and never overrides a shift the user chose.
 */
class ShiftState(private val doubleTapMs: Long = 300) {
    var state: Shift = Shift.OFF
        private set
    private var lastTapMs = Long.MIN_VALUE

    val isUpperCase: Boolean get() = state != Shift.OFF

    fun onShiftTap(timeMs: Long) {
        val doubleTap = lastTapMs != Long.MIN_VALUE && timeMs - lastTapMs <= doubleTapMs
        state = when (state) {
            Shift.OFF, Shift.AUTO -> Shift.ONCE
            Shift.ONCE -> if (doubleTap) Shift.LOCKED else Shift.OFF
            Shift.LOCKED -> Shift.OFF
        }
        lastTapMs = if (state == Shift.ONCE) timeMs else Long.MIN_VALUE
    }

    /** A letter was typed. */
    fun onLetter() {
        if (state == Shift.ONCE || state == Shift.AUTO) state = Shift.OFF
    }

    /** The editor's caps mode at the cursor changed. */
    fun onCapsMode(capsWanted: Boolean) {
        state = when {
            capsWanted && state == Shift.OFF -> Shift.AUTO
            !capsWanted && state == Shift.AUTO -> Shift.OFF
            else -> state
        }
    }

    fun reset() {
        state = Shift.OFF
        lastTapMs = Long.MIN_VALUE
    }
}
