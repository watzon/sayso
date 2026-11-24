package app.sayso.mobile.settings

import android.content.Context
import android.content.SharedPreferences
import androidx.core.content.edit
import app.sayso.mobile.engine.AutocorrectMode
import app.sayso.mobile.engine.EnginePreferences

data class KeyboardSettings(
    val haptics: Boolean = true,
    val autocorrect: AutocorrectMode = AutocorrectMode.NORMAL,
    val nextWordPrediction: Boolean = true,
    /** Two quick spaces after a word type ". ". */
    val doubleSpacePeriod: Boolean = true,
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
            doubleSpacePeriod = prefs.getBoolean(DOUBLE_SPACE_PERIOD, defaults.doubleSpacePeriod),
            diagnostics = prefs.getBoolean(DIAGNOSTICS, defaults.diagnostics),
        )
    }

    fun save(settings: KeyboardSettings) {
        prefs.edit {
            putBoolean(HAPTICS, settings.haptics)
            putString(AUTOCORRECT, settings.autocorrect.name)
            putBoolean(NEXT_WORD, settings.nextWordPrediction)
            putBoolean(DOUBLE_SPACE_PERIOD, settings.doubleSpacePeriod)
            putBoolean(DIAGNOSTICS, settings.diagnostics)
        }
    }

    private inline fun <reified E : Enum<E>> enumOr(name: String?, default: E): E =
        enumValues<E>().firstOrNull { it.name == name } ?: default

    private companion object {
        const val HAPTICS = "haptics"
        const val AUTOCORRECT = "autocorrect"
        const val NEXT_WORD = "next_word_prediction"
        const val DOUBLE_SPACE_PERIOD = "double_space_period"
        const val DIAGNOSTICS = "developer_diagnostics"
    }
}
