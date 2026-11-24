package app.sayso.mobile.ime

import android.content.Context
import android.content.SharedPreferences
import android.graphics.Paint
import android.os.Handler
import android.os.Looper
import androidx.core.content.edit
import app.sayso.mobile.emoji.Emoji
import app.sayso.mobile.emoji.EmojiCatalog
import app.sayso.mobile.emoji.EmojiRecents
import app.sayso.mobile.emoji.EmojiTones
import java.util.concurrent.Executors

/**
 * The emoji catalog for this process, read from the APK asset and filtered by
 * what the system font draws, plus the recent emoji and chosen skin tones.
 * The catalog loads once, off the main thread: checking about 3,500 glyphs
 * takes too long to do while the keyboard opens.
 */
class EmojiStore(context: Context) {
    private val app = context.applicationContext
    private val prefs: SharedPreferences = app.getSharedPreferences("emoji", Context.MODE_PRIVATE)

    val recents: EmojiRecents = EmojiRecents.decode(prefs.getString(RECENT, null))
    val tones: EmojiTones = EmojiTones.decode(prefs.getString(TONES, null))

    /** Calls [onReady] on the main thread with the catalog, at once when it is already loaded. */
    fun catalog(onReady: (EmojiCatalog) -> Unit) {
        loaded?.let { return onReady(it) }
        loader.execute {
            val catalog = loaded ?: load(app).also { loaded = it }
            main.post { onReady(catalog) }
        }
    }

    fun used(form: String) {
        recents.add(form)
        prefs.edit { putString(RECENT, recents.encode()) }
    }

    fun chooseTone(emoji: Emoji, form: String) {
        tones.choose(emoji, form)
        prefs.edit { putString(TONES, tones.encode()) }
    }

    private companion object {
        const val RECENT = "recent"
        const val TONES = "tones"
        const val ASSET = "emoji/emoji.txt"

        @Volatile
        var loaded: EmojiCatalog? = null
        val loader = Executors.newSingleThreadExecutor { Thread(it, "sayso-emoji").apply { isDaemon = true } }
        val main = Handler(Looper.getMainLooper())

        fun load(context: Context): EmojiCatalog {
            val paint = Paint()
            return context.assets.open(ASSET).bufferedReader().useLines { lines ->
                EmojiCatalog.parse(lines) { paint.hasGlyph(it) }
            }
        }
    }
}
