package app.sayso.mobile.ui

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontFamily
import app.sayso.mobile.settings.ThemeMode

// The Sumi ink palette of the desktop app (internal/theme/ink.go).
private val WarmPaper = lightColorScheme(
    primary = Color(0xFF2D3C8C),
    onPrimary = Color(0xFFF7F4EE),
    primaryContainer = Color(0xFFD7D7DE),
    onPrimaryContainer = Color(0xFF1D1B18),
    secondary = Color(0xFF1D1B18),
    onSecondary = Color(0xFFF7F4EE),
    secondaryContainer = Color(0xFFE2DCD0),
    onSecondaryContainer = Color(0xFF1D1B18),
    background = Color(0xFFF7F4EE),
    onBackground = Color(0xFF1D1B18),
    surface = Color(0xFFF7F4EE),
    onSurface = Color(0xFF1D1B18),
    surfaceVariant = Color(0xFFE2DCD0),
    onSurfaceVariant = Color(0xFF6A655C),
    surfaceContainerLowest = Color(0xFFFFFEFB),
    surfaceContainerLow = Color(0xFFFCFAF6),
    surfaceContainer = Color(0xFFFCFAF6),
    surfaceContainerHigh = Color(0xFFE9E4DA),
    surfaceContainerHighest = Color(0xFFE2DCD0),
    outline = Color(0xFF8E887D),
    outlineVariant = Color(0xFFD8D1C4),
    error = Color(0xFF8E2F22),
    onError = Color(0xFFF7F4EE),
    errorContainer = Color(0xFFF3E7DC),
    onErrorContainer = Color(0xFF8E2F22),
)

private val CrispInk = darkColorScheme(
    primary = Color(0xFF8D9BE6),
    onPrimary = Color(0xFF201F1C),
    primaryContainer = Color(0xFF343540),
    onPrimaryContainer = Color(0xFFECE5D8),
    secondary = Color(0xFFECE5D8),
    onSecondary = Color(0xFF201F1C),
    secondaryContainer = Color(0xFF292825),
    onSecondaryContainer = Color(0xFFECE5D8),
    background = Color(0xFF201F1C),
    onBackground = Color(0xFFECE5D8),
    surface = Color(0xFF201F1C),
    onSurface = Color(0xFFECE5D8),
    surfaceVariant = Color(0xFF292825),
    onSurfaceVariant = Color(0xFFA39D91),
    surfaceContainerLowest = Color(0xFF141311),
    surfaceContainerLow = Color(0xFF292825),
    surfaceContainer = Color(0xFF292825),
    surfaceContainerHigh = Color(0xFF171614),
    surfaceContainerHighest = Color(0xFF34322E),
    outline = Color(0xFF7D776C),
    outlineVariant = Color(0xFF34322E),
    error = Color(0xFFE08A7C),
    onError = Color(0xFF201F1C),
    errorContainer = Color(0xFF3A2420),
    onErrorContainer = Color(0xFFE08A7C),
)

@Composable
fun SaysoTheme(mode: ThemeMode, content: @Composable () -> Unit) {
    val dark = when (mode) {
        ThemeMode.SYSTEM -> isSystemInDarkTheme()
        ThemeMode.LIGHT -> false
        ThemeMode.DARK -> true
    }
    val base = Typography()
    // Serif headings stand in for the website's Fraunces; the fonts are not bundled.
    val typography = base.copy(
        headlineLarge = base.headlineLarge.copy(fontFamily = FontFamily.Serif),
        headlineMedium = base.headlineMedium.copy(fontFamily = FontFamily.Serif),
        titleLarge = base.titleLarge.copy(fontFamily = FontFamily.Serif),
    )
    MaterialTheme(colorScheme = if (dark) CrispInk else WarmPaper, typography = typography, content = content)
}
