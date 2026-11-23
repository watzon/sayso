package app.sayso.mobile.ime

import android.content.res.Configuration
import app.sayso.mobile.settings.ThemeMode

/** The Sumi ink palette of the desktop app (internal/theme/ink.go), as ARGB ints. */
data class KeyboardPalette(
    val dark: Boolean,
    val ground: Int,
    val key: Int,
    val modifierKey: Int,
    val keyShadow: Int,
    val pressedKey: Int,
    val ink: Int,
    val graphite: Int,
    val accent: Int,
    val onAccent: Int,
    val accentWash: Int,
    val rule: Int,
    val danger: Int,
    val success: Int,
) {
    companion object {
        val LIGHT = KeyboardPalette(
            dark = false,
            ground = 0xFFE9E4DA.toInt(),
            key = 0xFFFCFAF6.toInt(),
            modifierKey = 0xFFE2DCD0.toInt(),
            keyShadow = 0xFFCFC8BB.toInt(),
            pressedKey = 0xFFD8D1C4.toInt(),
            ink = 0xFF1D1B18.toInt(),
            graphite = 0xFF6A655C.toInt(),
            accent = 0xFF2D3C8C.toInt(),
            onAccent = 0xFFF7F4EE.toInt(),
            accentWash = 0xFFD7D7DE.toInt(),
            rule = 0xFFD8D1C4.toInt(),
            danger = 0xFF8E2F22.toInt(),
            success = 0xFF4E7A4A.toInt(),
        )
        val DARK = KeyboardPalette(
            dark = true,
            ground = 0xFF171614.toInt(),
            key = 0xFF292825.toInt(),
            modifierKey = 0xFF201F1C.toInt(),
            keyShadow = 0xFF0E0D0C.toInt(),
            pressedKey = 0xFF34322E.toInt(),
            ink = 0xFFECE5D8.toInt(),
            graphite = 0xFFA39D91.toInt(),
            accent = 0xFF8D9BE6.toInt(),
            onAccent = 0xFF201F1C.toInt(),
            accentWash = 0xFF343540.toInt(),
            rule = 0xFF34322E.toInt(),
            danger = 0xFFE08A7C.toInt(),
            success = 0xFF8FB98A.toInt(),
        )

        fun resolve(mode: ThemeMode, configuration: Configuration): KeyboardPalette = when (mode) {
            ThemeMode.LIGHT -> LIGHT
            ThemeMode.DARK -> DARK
            ThemeMode.SYSTEM ->
                if (configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK == Configuration.UI_MODE_NIGHT_YES) DARK else LIGHT
        }
    }
}
