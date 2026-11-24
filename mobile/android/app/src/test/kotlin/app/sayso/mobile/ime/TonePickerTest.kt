package app.sayso.mobile.ime

import org.junit.Assert.assertEquals
import org.junit.Test

class TonePickerTest {
    @Test
    fun thePickerStaysCenteredOnTheEmojiInsideThePanel() {
        assertEquals(400f, tonePickerLeft(anchorX = 500f, pickerWidth = 200f, panelWidth = 1080f, margin = 10f))
        assertEquals("clamped at the left", 10f, tonePickerLeft(50f, 200f, 1080f, 10f))
        assertEquals("clamped at the right", 870f, tonePickerLeft(1070f, 200f, 1080f, 10f))
    }

    /** A long-press crashed the keyboard: the picker measured itself while still GONE, at width 0. */
    @Test
    fun aPanelNarrowerThanThePickerDoesNotThrow() {
        assertEquals(0f, tonePickerLeft(75f, 760f, 0f, 10f))
        assertEquals(20f, tonePickerLeft(75f, 760f, 800f, 30f))
    }
}
