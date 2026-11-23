package app.sayso.mobile.engine

/**
 * The JNI functions of libsayso_jni.so, one per C function of
 * mobile/core/capi/sayso_core.h (see API.md there). Results come back in
 * arrays the caller owns and reuses, so the calls allocate nothing on the
 * Java heap. Functions that return Int give SAYSO_OK (0), a count, or a
 * negative SAYSO_ERR_* code.
 */
internal object SaysoCore {
    init {
        System.loadLibrary("sayso_jni")
    }

    const val OK = 0
    const val TEXT_CAP = 1024

    const val EDITOR_PRIVACY = 1
    const val EDITOR_AUTOCAP = 2
    const val EDITOR_NO_AUTOCORRECT = 4
    const val EDITOR_NO_SUGGEST = 8

    const val WORDS_ADDED = 1
    const val WORDS_LEARNED = 2

    const val ACTION_REPLACE = 1
    const val ACTION_CORRECTED = 1
    const val SUG_AUTOCORRECT = 2

    /** Indexes into the action array filled by the action calls. */
    const val A_KIND = 0
    const val A_DELETE_UTF16 = 2
    const val A_TEXT_LEN = 3
    const val A_FLAGS = 4
    const val ACTION_INTS = 5

    /** The most keys a layout may have (touch.MaxKeys). */
    const val MAX_KEYS = 96
    const val MAX_CANDIDATES = 8

    /**
     * Diagnostics layout. ints: would_correct, literal_is_word, key_count,
     * cand_count, last touch code, key index, flags, then 2 per key (code,
     * samples) from D_KEYS, then 4 per candidate (text offset, text length,
     * edits, kind) from D_CANDS. floats: margin, literal total, then 3 per key
     * (expansion, dx, dy) from DF_KEYS, then 3 per candidate (total, spatial,
     * lm) from DF_CANDS.
     */
    const val D_KEYS = 8
    const val D_CANDS = D_KEYS + 2 * MAX_KEYS
    const val DIAG_INTS = D_CANDS + 4 * MAX_CANDIDATES
    const val DF_KEYS = 2
    const val DF_CANDS = DF_KEYS + 3 * MAX_KEYS
    const val DIAG_FLOATS = DF_CANDS + 3 * MAX_CANDIDATES

    @JvmStatic external fun version(): String

    /** The last error of an engine, or of the last failed create when [handle] is 0. */
    @JvmStatic external fun lastError(handle: Long): String

    /** A handle (> 0), or a negative SAYSO_ERR_* code. */
    @JvmStatic external fun create(modelPath: ByteArray, journalPath: ByteArray): Long

    /** A handle (> 0), or a negative SAYSO_ERR_* code. */
    @JvmStatic external fun createFd(fd: Int, offset: Long, length: Long, journalPath: ByteArray): Long

    @JvmStatic external fun destroy(handle: Long)

    /** [rects] holds left, top, width, height for each of the [count] keys. */
    @JvmStatic external fun setLayout(handle: Long, codes: IntArray, rects: FloatArray, count: Int): Int

    @JvmStatic external fun setEditorFlags(handle: Long, flags: Int): Int

    @JvmStatic external fun setPreferences(handle: Long, autocorrect: Int, predictions: Boolean): Int

    @JvmStatic external fun setContext(handle: Long, text: ByteArray): Int

    /** The letter typed, in its case, or 0 when the touch typed no letter (or failed). */
    @JvmStatic external fun onTouch(handle: Long, x: Float, y: Float, timeMs: Long, shifted: Boolean): Int

    @JvmStatic external fun typeChar(handle: Long, codePoint: Int, action: IntArray, text: ByteArray): Int

    @JvmStatic external fun backspace(handle: Long, action: IntArray, text: ByteArray): Int

    @JvmStatic external fun revertLastAutocorrect(handle: Long, action: IntArray, text: ByteArray): Int

    /** The count; for each, ints[3i..] = text offset, text length, flags and floats[i] = score. */
    @JvmStatic external fun suggestions(handle: Long, max: Int, ints: IntArray, floats: FloatArray, text: ByteArray): Int

    @JvmStatic external fun pickSuggestion(handle: Long, index: Int, action: IntArray, text: ByteArray): Int

    /** The word's length in bytes; the bytes that fit go to [text]. */
    @JvmStatic external fun currentWord(handle: Long, text: ByteArray): Int

    @JvmStatic external fun addWord(handle: Long, word: ByteArray): Int

    @JvmStatic external fun removeWord(handle: Long, word: ByteArray): Int

    @JvmStatic external fun addReplacement(handle: Long, from: ByteArray, to: ByteArray, caseSensitive: Boolean): Int

    @JvmStatic external fun removeReplacement(handle: Long, from: ByteArray): Int

    @JvmStatic external fun resetLearned(handle: Long): Int

    /** The bytes the list needs; written to [out] only when they fit. [out] may be null. */
    @JvmStatic external fun listWords(handle: Long, which: Int, out: ByteArray?): Int

    /** The bytes the list needs; written to [out] only when they fit. [out] may be null. */
    @JvmStatic external fun listReplacements(handle: Long, out: ByteArray?): Int

    @JvmStatic external fun diagnostics(handle: Long, ints: IntArray, floats: FloatArray, text: ByteArray, keyCap: Int, candCap: Int): Int
}
