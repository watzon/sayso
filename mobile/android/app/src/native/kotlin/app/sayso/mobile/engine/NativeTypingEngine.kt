package app.sayso.mobile.engine

import android.content.Context
import android.util.Log
import java.io.File
import java.io.IOException

/**
 * [TypingEngine] on the shared Go typing core (libsayso_core.so) through
 * [SaysoCore]. The core keeps its own mirror of the text before the cursor,
 * the word being typed, and what it learns, in a journal file. Calls come
 * from the main thread; the reused result arrays are not shared across
 * threads.
 */
class NativeTypingEngine internal constructor(internal val handle: Long) : TypingEngine {
    private val action = IntArray(SaysoCore.ACTION_INTS)
    private val text = ByteArray(SaysoCore.TEXT_CAP)
    private val sugInts = IntArray(3 * MAX_SUGGESTIONS)
    private val sugFloats = FloatArray(MAX_SUGGESTIONS)
    private val diagInts = IntArray(SaysoCore.DIAG_INTS)
    private val diagFloats = FloatArray(SaysoCore.DIAG_FLOATS)

    private var keys: List<EngineKey> = emptyList()
    private var flags = SaysoCore.EDITOR_AUTOCAP

    /** The separator of the last autocorrect, so a revert from the strip can put it back. */
    private var lastSeparator: Char? = null

    /** What the last suggestions() call said a commit would write, for the diagnostics. */
    private var autocorrectText: String? = null

    override fun setLayout(layout: EngineLayout) {
        val n = minOf(layout.keys.size, SaysoCore.MAX_KEYS)
        if (n == 0) return
        val codes = IntArray(n)
        val rects = FloatArray(4 * n)
        for (i in 0 until n) {
            val k = layout.keys[i]
            codes[i] = k.code
            rects[4 * i] = k.left
            rects[4 * i + 1] = k.top
            rects[4 * i + 2] = k.right - k.left
            rects[4 * i + 3] = k.bottom - k.top
        }
        if (ok(SaysoCore.setLayout(handle, codes, rects, n), "setLayout")) keys = layout.keys.take(n)
    }

    override fun setEditorFlags(flags: EditorFlags) {
        var f = 0
        if (flags.privacy) f = f or SaysoCore.EDITOR_PRIVACY
        if (flags.autoCapitalize) f = f or SaysoCore.EDITOR_AUTOCAP
        if (!flags.suggestionsAllowed) f = f or SaysoCore.EDITOR_NO_AUTOCORRECT or SaysoCore.EDITOR_NO_SUGGEST
        this.flags = f
        ok(SaysoCore.setEditorFlags(handle, f), "setEditorFlags")
    }

    override fun setPreferences(preferences: EnginePreferences) {
        ok(SaysoCore.setPreferences(handle, autocorrectLevel(preferences.autocorrect), preferences.nextWordPrediction), "setPreferences")
    }

    override fun setContext(textBeforeCursor: String) {
        lastSeparator = null
        ok(SaysoCore.setContext(handle, textBeforeCursor.encodeToByteArray()), "setContext")
    }

    // The hot path: primitives in and out, nothing allocated.
    override fun onTouch(x: Float, y: Float, timeMs: Long, shifted: Boolean): Int =
        SaysoCore.onTouch(handle, x, y, timeMs, shifted)

    override fun onText(text: String): CommitAction {
        var result = CommitAction(0, "")
        var i = 0
        while (i < text.length) {
            val cp = text.codePointAt(i)
            val n = Character.charCount(cp)
            result = result.then(typeChar(cp) ?: CommitAction(0, text.substring(i, i + n)))
            i += n
        }
        return result
    }

    override fun onBackspace(): CommitAction? {
        lastSeparator = null
        return replaceOrNull(SaysoCore.backspace(handle, action, text), "backspace")
    }

    override fun onSpaceOrPunct(char: Char): CommitAction {
        val typed = currentWord()
        val replace = typeChar(char.code) ?: return CommitAction(0, char.toString())
        val corrected = action[SaysoCore.A_FLAGS] and SaysoCore.ACTION_CORRECTED != 0
        lastSeparator = if (corrected) char else null
        return replace.copy(wasAutocorrect = corrected, original = if (corrected) typed else "")
    }

    override fun suggestions(): List<Suggestion> {
        val n = SaysoCore.suggestions(handle, MAX_SUGGESTIONS, sugInts, sugFloats, text)
        autocorrectText = null
        if (n <= 0) {
            ok(n, "suggestions")
            return emptyList()
        }
        return List(n) { i ->
            val auto = sugInts[3 * i + 2] and SaysoCore.SUG_AUTOCORRECT != 0
            val s = Suggestion(decode(sugInts[3 * i], sugInts[3 * i + 1]), sugFloats[i], isAutoCommit = auto)
            if (auto) autocorrectText = s.text
            s
        }
    }

    override fun pickSuggestion(index: Int): CommitAction {
        lastSeparator = null
        return replaceOrNull(SaysoCore.pickSuggestion(handle, index, action, text), "pickSuggestion") ?: CommitAction(0, "")
    }

    override fun revertLastAutocorrect(): CommitAction? {
        val separator = lastSeparator
        lastSeparator = null
        val revert = replaceOrNull(SaysoCore.revertLastAutocorrect(handle, action, text), "revert") ?: return null
        // The core reopens the typed word; its separator commits it as typed.
        if (separator == null) return revert
        return revert.then(typeChar(separator.code) ?: CommitAction(0, separator.toString()))
    }

    override fun personalWords(): List<String> = list { SaysoCore.listWords(handle, SaysoCore.WORDS_ADDED or SaysoCore.WORDS_LEARNED, it) }

    override fun addPersonalWord(word: String) {
        val trimmed = word.trim()
        if (trimmed.isEmpty()) return
        userEdit { ok(SaysoCore.addWord(handle, trimmed.encodeToByteArray()), "addWord") }
    }

    override fun removePersonalWord(word: String) {
        userEdit { ok(SaysoCore.removeWord(handle, word.encodeToByteArray()), "removeWord") }
    }

    override fun replacements(): List<Replacement> = replacementRecords().map { Replacement(it.from, it.to) }

    override fun putReplacement(replacement: Replacement) {
        val shortcut = replacement.shortcut.trim().lowercase()
        val expansion = replacement.expansion.trim()
        if (shortcut.isEmpty() || expansion.isEmpty() || shortcut.any { !isWordChar(it) }) return
        userEdit {
            ok(SaysoCore.addReplacement(handle, shortcut.encodeToByteArray(), expansion.encodeToByteArray(), false), "addReplacement")
        }
    }

    override fun removeReplacement(shortcut: String) {
        userEdit { ok(SaysoCore.removeReplacement(handle, shortcut.encodeToByteArray()), "removeReplacement") }
    }

    override fun resetLearnedData() {
        // The core forgets everything; put back what the user added by hand.
        val words = list { SaysoCore.listWords(handle, SaysoCore.WORDS_ADDED, it) }
        val replacements = replacementRecords()
        userEdit {
            ok(SaysoCore.resetLearned(handle), "resetLearned")
            for (w in words) ok(SaysoCore.addWord(handle, w.encodeToByteArray()), "addWord")
            for (r in replacements) {
                ok(SaysoCore.addReplacement(handle, r.from.encodeToByteArray(), r.to.encodeToByteArray(), r.caseSensitive), "addReplacement")
            }
        }
        lastSeparator = null
    }

    override fun diagnostics(): Diagnostics {
        val rc = SaysoCore.diagnostics(handle, diagInts, diagFloats, text, keys.size, SaysoCore.MAX_CANDIDATES)
        if (!ok(rc, "diagnostics")) return Diagnostics(emptyList(), emptyList(), 0f, 0f)
        val wouldCorrect = diagInts[0] != 0
        val keyCount = minOf(diagInts[2], keys.size)
        val candCount = diagInts[3]
        val keyDiagnostics = List(keyCount) { i ->
            val k = keys[i]
            val e = diagFloats[SaysoCore.DF_KEYS + 3 * i]
            val dx = diagFloats[SaysoCore.DF_KEYS + 3 * i + 1]
            val dy = diagFloats[SaysoCore.DF_KEYS + 3 * i + 2]
            KeyDiagnostics(
                code = k.code,
                left = k.left - e + dx,
                top = k.top - e + dy,
                right = k.right + e + dx,
                bottom = k.bottom + e + dy,
                learnedDx = dx,
                learnedDy = dy,
                expansion = e,
                samples = diagInts[SaysoCore.D_KEYS + 2 * i + 1],
            )
        }
        val details = List(candCount) { i ->
            val c = SaysoCore.D_CANDS + 4 * i
            val f = SaysoCore.DF_CANDS + 3 * i
            CandidateDiagnostics(decode(diagInts[c], diagInts[c + 1]), diagFloats[f], diagFloats[f + 1], diagFloats[f + 2], diagInts[c + 2])
        }
        val auto = autocorrectText
        val candidates = details.map {
            Suggestion(it.text, it.total, isAutoCommit = wouldCorrect && auto != null && it.text.equals(auto, ignoreCase = true))
        }
        // The core's margin is measured from the autocorrect bar: it corrects at 0 and above.
        return Diagnostics(keyDiagnostics, candidates, diagFloats[0], 0f, details)
    }

    /** Closes the engine; the object must not be used after. */
    fun close() = SaysoCore.destroy(handle)

    private fun typeChar(codePoint: Int): CommitAction? = replaceOrNull(SaysoCore.typeChar(handle, codePoint, action, text), "typeChar")

    /** The action just written to [action] and [text], or null for SAYSO_ACTION_NONE (and errors). */
    private fun replaceOrNull(rc: Int, call: String): CommitAction? {
        if (!ok(rc, call) || action[SaysoCore.A_KIND] != SaysoCore.ACTION_REPLACE) return null
        return CommitAction(action[SaysoCore.A_DELETE_UTF16], decode(0, action[SaysoCore.A_TEXT_LEN]))
    }

    private fun currentWord(): String {
        val n = SaysoCore.currentWord(handle, text)
        return if (n > 0) decode(0, minOf(n, text.size)) else ""
    }

    private fun decode(offset: Int, length: Int) = String(text, offset, length, Charsets.UTF_8)

    /** A NUL-separated list from the core, sized by a first call. */
    private fun list(call: (ByteArray?) -> Int): List<String> {
        val need = call(null)
        if (need <= 0) return emptyList()
        val buf = ByteArray(need)
        if (call(buf) != need) return emptyList()
        return splitNul(buf)
    }

    private fun replacementRecords(): List<ReplacementRecord> =
        list { SaysoCore.listReplacements(handle, it) }.chunked(3).filter { it.size == 3 }.map { ReplacementRecord(it[0], it[1], it[2] == "1") }

    /**
     * Dictionary edits from the companion app are kept even when the last
     * field the keyboard saw was private: the core writes nothing in privacy
     * mode, so lift it for the edit.
     */
    private inline fun userEdit(block: () -> Unit) {
        val inPrivate = flags and SaysoCore.EDITOR_PRIVACY != 0
        if (inPrivate) SaysoCore.setEditorFlags(handle, flags and SaysoCore.EDITOR_PRIVACY.inv())
        try {
            block()
        } finally {
            if (inPrivate) SaysoCore.setEditorFlags(handle, flags)
        }
    }

    private fun ok(rc: Int, call: String): Boolean {
        if (rc >= SaysoCore.OK) return true
        Log.w(TAG, "$call: $rc ${SaysoCore.lastError(handle)}")
        return false
    }

    private data class ReplacementRecord(val from: String, val to: String, val caseSensitive: Boolean)

    companion object {
        private const val TAG = "SaysoEngine"
        private const val MAX_SUGGESTIONS = 3
        const val MODEL_ASSET = "en-US.sslm"
        const val JOURNAL_FILE = "typing.journal"

        /**
         * Opens the bundled model straight from the APK (it is stored
         * uncompressed, so the core maps it in place). If that fails, the
         * model is copied to app storage once and opened from there.
         */
        fun create(context: Context): NativeTypingEngine {
            val journal = File(context.filesDir, JOURNAL_FILE).path
            val h = openAsset(context, journal) ?: openCopy(context, journal)
            return NativeTypingEngine(h)
        }

        /** Opens a model file directly (tests, and the copy fallback). */
        fun open(modelPath: String, journalPath: String): NativeTypingEngine {
            val h = SaysoCore.create(modelPath.encodeToByteArray(), journalPath.encodeToByteArray())
            if (h <= 0) throw IllegalStateException("sayso core: ${SaysoCore.lastError(0)} ($h)")
            return NativeTypingEngine(h)
        }

        private fun openAsset(context: Context, journal: String): Long? = try {
            context.assets.openFd(MODEL_ASSET).use { afd ->
                val h = SaysoCore.createFd(afd.parcelFileDescriptor.fd, afd.startOffset, afd.length, journal.encodeToByteArray())
                if (h > 0) h else null.also { Log.w(TAG, "model from the APK: ${SaysoCore.lastError(0)} ($h)") }
            }
        } catch (e: IOException) {
            Log.w(TAG, "model from the APK: $e")
            null
        }

        private fun openCopy(context: Context, journal: String): Long {
            val copy = File(context.noBackupFilesDir, MODEL_ASSET)
            val installed = context.packageManager.getPackageInfo(context.packageName, 0).lastUpdateTime
            if (!copy.exists() || copy.lastModified() < installed) {
                val tmp = File(copy.path + ".tmp")
                context.assets.open(MODEL_ASSET).use { input -> tmp.outputStream().use { input.copyTo(it) } }
                if (!tmp.renameTo(copy)) throw IOException("cannot write $copy")
            }
            val h = SaysoCore.create(copy.path.encodeToByteArray(), journal.encodeToByteArray())
            if (h <= 0) throw IllegalStateException("sayso core: ${SaysoCore.lastError(0)} ($h)")
            return h
        }

        internal fun autocorrectLevel(mode: AutocorrectMode) = when (mode) {
            AutocorrectMode.OFF -> 0
            AutocorrectMode.NORMAL -> 1
            AutocorrectMode.STRICT -> 2
        }

        internal fun splitNul(buf: ByteArray): List<String> {
            val out = ArrayList<String>()
            var start = 0
            for (i in buf.indices) {
                if (buf[i] == 0.toByte()) {
                    out += String(buf, start, i - start, Charsets.UTF_8)
                    start = i + 1
                }
            }
            return out
        }
    }
}
