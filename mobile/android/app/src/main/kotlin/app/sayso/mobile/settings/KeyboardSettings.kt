package app.sayso.mobile.settings

import android.content.Context
import android.content.SharedPreferences
import androidx.core.content.edit
import app.sayso.mobile.engine.AutocorrectMode
import app.sayso.mobile.engine.EnginePreferences

enum class ThemeMode { SYSTEM, LIGHT, DARK }

data class KeyboardSettings(
    val haptics: Boolean = true,
    val autocorrect: AutocorrectMode = AutocorrectMode.NORMAL,
    val nextWordPrediction: Boolean = true,
    val theme: ThemeMode = ThemeMode.SYSTEM,
    val diagnostics: Boolean = false,
) {
    fun enginePreferences() = EnginePreferences(autocorrect, nextWordPrediction)
}

/** Settings written by the companion app and read by the keyboard each time it opens. */
class KeyboardSettingsStore(context: Context) {
    private val prefs: SharedPreferences = context.applicationContext.getSharedPreferences("keyboard", Context.MODE_PRIVATE)

    fun load(): KeyboardSettings {
        val defaults = KeyboardSettings()
        return KeyboardSettings(
            haptics = prefs.getBoolean(HAPTICS, defaults.haptics),
            autocorrect = enumOr(prefs.getString(AUTOCORRECT, null), defaults.autocorrect),
            nextWordPrediction = prefs.getBoolean(NEXT_WORD, defaults.nextWordPrediction),
            theme = enumOr(prefs.getString(THEME, null), defaults.theme),
            diagnostics = prefs.getBoolean(DIAGNOSTICS, defaults.diagnostics),
        )
    }

    fun save(settings: KeyboardSettings) {
        prefs.edit {
            putBoolean(HAPTICS, settings.haptics)
            putString(AUTOCORRECT, settings.autocorrect.name)
            putBoolean(NEXT_WORD, settings.nextWordPrediction)
            putString(THEME, settings.theme.name)
            putBoolean(DIAGNOSTICS, settings.diagnostics)
        }
    }

    private inline fun <reified E : Enum<E>> enumOr(name: String?, default: E): E =
        enumValues<E>().firstOrNull { it.name == name } ?: default

    private companion object {
        const val HAPTICS = "haptics"
        const val AUTOCORRECT = "autocorrect"
        const val NEXT_WORD = "next_word_prediction"
        const val THEME = "theme"
        const val DIAGNOSTICS = "developer_diagnostics"
    }
}
