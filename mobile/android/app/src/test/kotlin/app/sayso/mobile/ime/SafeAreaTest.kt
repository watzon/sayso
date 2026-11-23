package app.sayso.mobile.ime

import androidx.core.graphics.Insets
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

class SafeAreaTest {
    /**
     * The insets an API 36 emulator gave the input view with gesture navigation
     * (1080x2400, 420dpi). navigationBars() was 63px, the gesture handle;
     * padding by it left the keyboard's own hide and switch buttons (126px)
     * drawn over the bottom row of keys.
     */
    @Test
    fun bottomClearsTheKeyboardNavigationButtons() {
        val safe = keyboardSafeArea(
            systemBars = Insets.of(0, 0, 0, 126),
            tappable = Insets.of(0, 0, 0, 126),
            cutout = Insets.NONE,
        )
        assertEquals(Insets.of(0, 0, 0, 126), safe)
    }

    @Test
    fun sidesClearLandscapeBarsAndCutouts() {
        val safe = keyboardSafeArea(
            systemBars = Insets.of(0, 63, 0, 0),
            tappable = Insets.of(0, 0, 132, 0),
            cutout = Insets.of(88, 0, 0, 0),
        )
        assertEquals(Insets.of(88, 0, 132, 0), safe)
    }

    @Test
    fun theServicePadsBySafeAreaNotNavigationBarsAlone() {
        val service = File("src/main/kotlin/app/sayso/mobile/ime/SaysoImeService.kt").readText()
        assertTrue(service.contains("keyboardSafeArea(insets)"))
        assertFalse(service.contains("Type.navigationBars()"))
    }
}
