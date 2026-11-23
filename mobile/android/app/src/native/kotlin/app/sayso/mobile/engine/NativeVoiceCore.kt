package app.sayso.mobile.engine

import android.util.Log

/**
 * [VoiceCore] on the shared Go core through [SaysoCore], on the same engine
 * handle as [NativeTypingEngine]: post-processing reads the keyboard's
 * personal words and replacements, and styles and history go to
 * voice.journal next to the typing journal.
 *
 * Threads: this class holds no state of its own, so any thread may call
 * it. The core serializes calls on one engine; [enhance] and
 * [transcribeCloud] copy what they need and let go of the engine before
 * they block on the network, so the keyboard's calls on the main thread
 * never wait for them. Run those two on a background thread. If the
 * typing engine is closed, calls fail softly: post-processing gives back
 * the trimmed transcript, lists are empty, and recording does nothing.
 */
class NativeVoiceCore internal constructor(private val handle: Long) : VoiceCore {

    override fun postProcess(raw: String, textBeforeCursor: String, textAfterCursor: String): String {
        val out = SaysoCore.postProcess(handle, raw.encodeToByteArray(), textBeforeCursor.encodeToByteArray(), textAfterCursor.encodeToByteArray())
        if (out == null) {
            Log.w(TAG, "postProcess: ${SaysoCore.lastError(handle)}")
            return raw.trim()
        }
        return out.decodeToString()
    }

    override fun styles(): List<VoiceStyle> =
        list { SaysoCore.listStyles(handle, it) }.chunked(4).filter { it.size == 4 }.map { VoiceStyle(it[0], it[1], it[2], builtIn = it[3] == "1") }

    override fun putCustomStyle(style: VoiceStyle) {
        val rc = SaysoCore.putStyle(handle, style.id.encodeToByteArray(), style.name.encodeToByteArray(), style.prompt.encodeToByteArray())
        if (rc == SaysoCore.ERR_ARG) throw IllegalArgumentException("style ${style.id}: ${SaysoCore.lastError(handle)}")
        ok(rc, "putStyle")
    }

    override fun removeCustomStyle(id: String) {
        // A built-in id is refused by the core and, as in FakeVoiceCore, ignored here.
        val rc = SaysoCore.removeStyle(handle, id.encodeToByteArray())
        if (rc != SaysoCore.ERR_ARG) ok(rc, "removeStyle")
    }

    override fun enhance(styleId: String, text: String, cloud: CloudConfig, timeoutMs: Int): EnhanceResult {
        if (cloud.styleProvider == CloudStyleProvider.OFF) return EnhanceResult(text, enhanced = false)
        val result = arrayOfNulls<ByteArray>(2)
        val rc = SaysoCore.enhance(
            handle, cloud.styleProvider.ordinal, cloud.styleApiKey.encodeToByteArray(), cloud.styleBaseUrl.encodeToByteArray(),
            cloud.styleModel.encodeToByteArray(), timeoutMs, styleId.encodeToByteArray(), text.encodeToByteArray(), result,
        )
        if (rc < 0) return EnhanceResult(text, enhanced = false, error = "core error $rc: ${SaysoCore.lastError(handle)}")
        val error = result[1]?.decodeToString()
        if (error != null) Log.w(TAG, "enhance ($styleId): $error")
        return EnhanceResult(result[0]?.decodeToString() ?: text, enhanced = rc == 1, error = error)
    }

    override fun transcribeCloud(pcm16k: ShortArray, cloud: CloudConfig, timeoutMs: Int): String? {
        if (cloud.asrProvider == CloudAsrProvider.OFF) return null
        val result = arrayOfNulls<ByteArray>(2)
        val rc = SaysoCore.transcribeCloud(
            handle, cloud.asrProvider.ordinal, cloud.asrApiKey.encodeToByteArray(), cloud.asrBaseUrl.encodeToByteArray(),
            cloud.asrModel.encodeToByteArray(), timeoutMs, pcm16k, result,
        )
        if (rc != SaysoCore.OK) {
            Log.w(TAG, "transcribeCloud: ${result[1]?.decodeToString() ?: "core error $rc"}")
            return null
        }
        return result[0]?.decodeToString()
    }

    override fun recordHistory(entry: VoiceHistoryEntry, sensitive: Boolean) {
        if (sensitive) return
        val id = SaysoCore.historyAdd(
            handle, entry.timestampMs, entry.durationMs, entry.engineId.encodeToByteArray(), entry.styleId.encodeToByteArray(),
            entry.rawText.encodeToByteArray(), entry.finalText.encodeToByteArray(), sensitive,
        )
        if (id < 0) Log.w(TAG, "recordHistory: $id ${SaysoCore.lastError(handle)}")
    }

    override fun history(limit: Int): List<VoiceHistoryEntry> =
        list { SaysoCore.historyList(handle, limit, it) }.chunked(HISTORY_FIELDS).filter { it.size == HISTORY_FIELDS }.mapNotNull { f ->
            val id = f[0].toLongOrNull() ?: return@mapNotNull null
            VoiceHistoryEntry(id, f[1].toLongOrNull() ?: 0, f[2].toLongOrNull() ?: 0, f[3], f[4], f[5], f[6])
        }

    override fun deleteHistory(id: Long) = ok(SaysoCore.historyDelete(handle, id), "deleteHistory")

    override fun clearHistory() = ok(SaysoCore.historyClear(handle), "clearHistory")

    /**
     * A NUL-separated list from the core, sized by a first call. Another
     * thread may grow it between the two calls: then size it again.
     */
    private fun list(call: (ByteArray?) -> Int): List<String> {
        var need = call(null)
        repeat(4) {
            if (need <= 0) return emptyList()
            val buf = ByteArray(need)
            val got = call(buf)
            if (got == need) return NativeTypingEngine.splitNul(buf)
            need = got
        }
        return emptyList()
    }

    private fun ok(rc: Int, call: String) {
        if (rc < SaysoCore.OK) Log.w(TAG, "$call: $rc ${SaysoCore.lastError(handle)}")
    }

    private companion object {
        const val TAG = "SaysoVoice"
        const val HISTORY_FIELDS = 7
    }
}
