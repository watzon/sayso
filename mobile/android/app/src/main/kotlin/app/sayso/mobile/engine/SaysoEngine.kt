package app.sayso.mobile.engine

import android.content.Context

/**
 * The one engine of the app process, shared by the keyboard and the companion
 * app so dictionary edits reach the keyboard at once. [createTypingEngine]
 * lives in src/fake or src/native, picked by the saysoNative Gradle property.
 */
object SaysoEngine {
    private var engine: TypingEngine? = null

    fun get(context: Context): TypingEngine =
        engine ?: createTypingEngine(context.applicationContext).also { engine = it }
}
