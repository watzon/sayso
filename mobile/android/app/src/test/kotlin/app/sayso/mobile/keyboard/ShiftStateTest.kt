package app.sayso.mobile.keyboard

import org.junit.Assert.assertEquals
import org.junit.Test

class ShiftStateTest {
    private val shift = ShiftState(doubleTapMs = 300)

    @Test
    fun tapShiftsOneLetter() {
        shift.onShiftTap(0)
        assertEquals(Shift.ONCE, shift.state)
        shift.onLetter()
        assertEquals(Shift.OFF, shift.state)
    }

    @Test
    fun secondTapLaterTurnsShiftOff() {
        shift.onShiftTap(0)
        shift.onShiftTap(1000)
        assertEquals(Shift.OFF, shift.state)
    }

    @Test
    fun doubleTapLocksAndSurvivesLettersUntilTapped() {
        shift.onShiftTap(0)
        shift.onShiftTap(200)
        assertEquals(Shift.LOCKED, shift.state)
        shift.onLetter()
        shift.onCapsMode(false)
        assertEquals(Shift.LOCKED, shift.state)
        shift.onShiftTap(5000)
        assertEquals(Shift.OFF, shift.state)
    }

    @Test
    fun tapAfterUnlockStartsAFreshDoubleTapWindow() {
        shift.onShiftTap(0)
        shift.onShiftTap(100) // locked
        shift.onShiftTap(200) // off
        shift.onShiftTap(300) // once, not locked: the unlocking tap does not count
        assertEquals(Shift.ONCE, shift.state)
    }

    @Test
    fun autoCapFollowsTheEditorButNeverOverridesTheUser() {
        shift.onCapsMode(true)
        assertEquals(Shift.AUTO, shift.state)
        shift.onCapsMode(false)
        assertEquals(Shift.OFF, shift.state)

        shift.onShiftTap(0)
        shift.onCapsMode(false)
        assertEquals(Shift.ONCE, shift.state)
    }

    @Test
    fun tapOnAutoShiftGivesOneShift() {
        shift.onCapsMode(true)
        shift.onShiftTap(0)
        assertEquals(Shift.ONCE, shift.state)
        shift.onLetter()
        assertEquals(Shift.OFF, shift.state)
    }
}
