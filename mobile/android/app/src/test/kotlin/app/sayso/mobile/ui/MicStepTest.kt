package app.sayso.mobile.ui

import org.junit.Assert.assertEquals
import org.junit.Test

class MicStepTest {
    @Test
    fun grantedWins() {
        assertEquals(MicStep.GRANTED, micStep(granted = true, askedBefore = true, showRationale = false))
    }

    @Test
    fun noRationaleBeforeTheFirstRequestStillAsks() {
        assertEquals(MicStep.ASK, micStep(granted = false, askedBefore = false, showRationale = false))
    }

    @Test
    fun firstDenialAsksAgain() {
        assertEquals(MicStep.ASK, micStep(granted = false, askedBefore = true, showRationale = true))
    }

    @Test
    fun denialWithoutRationaleIsBlocked() {
        assertEquals(MicStep.BLOCKED, micStep(granted = false, askedBefore = true, showRationale = false))
    }
}
