package app.sayso.mobile.asr

import android.app.Service
import android.content.Intent
import android.os.Bundle
import android.os.Handler
import android.os.HandlerThread
import android.os.IBinder
import android.os.Message
import android.os.Messenger
import android.os.RemoteException
import android.util.Log
import app.sayso.mobile.voice.Pcm
import app.sayso.mobile.voice.normalizeTranscript
import java.io.File

/**
 * The Messenger protocol between the keyboard ([AsrClient]) and [AsrService].
 * Every message carries the dictation's session number in arg1; the service
 * drops audio for any session but the current one.
 */
internal object AsrProtocol {
    /** Keyboard to service. Data: the model ids and folders, see [models]. */
    const val START = 1

    /** Data: [PCM], a ShortArray of 16 kHz mono samples. */
    const val AUDIO = 2
    const val FINISH = 3
    const val CANCEL = 4

    /** Service to keyboard. Data: [TEXT]. */
    const val PARTIAL = 10
    const val FINAL = 11

    /** Data: [TEXT], the reason. */
    const val ERROR = 12

    const val PCM = "pcm"
    const val TEXT = "text"
    private const val FINAL_ID = "final_id"
    private const val FINAL_DIR = "final_dir"
    private const val LIVE_ID = "live_id"
    private const val LIVE_DIR = "live_dir"

    fun startData(final: ModelOnDisk?, live: ModelOnDisk?) = Bundle().apply {
        final?.let { putString(FINAL_ID, it.model.id); putString(FINAL_DIR, it.dir.absolutePath) }
        live?.let { putString(LIVE_ID, it.model.id); putString(LIVE_DIR, it.dir.absolutePath) }
    }

    /** The models of a [START] message: the final model and the live model. */
    fun models(data: Bundle): Pair<ModelOnDisk?, ModelOnDisk?> {
        fun read(idKey: String, dirKey: String): ModelOnDisk? {
            val model = ModelCatalog.byId(data.getString(idKey)) ?: return null
            val dir = data.getString(dirKey) ?: return null
            return ModelOnDisk(model, File(dir))
        }
        return read(FINAL_ID, FINAL_DIR) to read(LIVE_ID, LIVE_DIR)
    }
}

/**
 * Hosts local speech recognition in its own process (android:process=":asr").
 * Models load on the first dictation and stay loaded while the keyboard keeps
 * the service bound. All work runs on one background thread, in message order.
 */
class AsrService : Service() {
    private lateinit var thread: HandlerThread
    private lateinit var messenger: Messenger
    private var asr: LocalAsr? = null
    private var stream: AsrStream? = null
    private var session = 0
    private var client: Messenger? = null
    private var lastPartial = ""

    override fun onCreate() {
        super.onCreate()
        thread = HandlerThread("sayso-asr").apply { start() }
        messenger = Messenger(Handler(thread.looper, ::handle))
    }

    override fun onBind(intent: Intent?): IBinder = messenger.binder

    override fun onDestroy() {
        Handler(thread.looper).post {
            closeStream()
            asr?.release()
            asr = null
        }
        thread.quitSafely()
        super.onDestroy()
    }

    private fun handle(msg: Message): Boolean {
        when (msg.what) {
            AsrProtocol.START -> start(msg)
            AsrProtocol.AUDIO -> if (msg.arg1 == session) audio(msg.data.getShortArray(AsrProtocol.PCM) ?: return true)
            AsrProtocol.FINISH -> if (msg.arg1 == session) finish()
            AsrProtocol.CANCEL -> if (msg.arg1 == session) closeStream()
        }
        return true
    }

    private fun start(msg: Message) {
        closeStream()
        session = msg.arg1
        client = msg.replyTo
        lastPartial = ""
        val (final, live) = AsrProtocol.models(msg.data)
        try {
            val engine = asr ?: createLocalAsr()?.also { asr = it } ?: return reply(AsrProtocol.ERROR, "This build has no local speech engine")
            if (final == null && live == null) return reply(AsrProtocol.ERROR, "No local model is installed")
            stream = engine.open(final, live)
        } catch (e: Throwable) {
            // Includes OutOfMemoryError: a model too big for this device must not kill the process.
            Log.w(TAG, "model load failed", e)
            asr?.release()
            reply(AsrProtocol.ERROR, "The speech model could not load")
        }
    }

    private fun audio(pcm: ShortArray) {
        val s = stream ?: return
        try {
            s.accept(Pcm.toFloats(pcm))
            val text = s.partial()?.let(::normalizeTranscript) ?: return
            if (text != lastPartial) {
                lastPartial = text
                reply(AsrProtocol.PARTIAL, text)
            }
        } catch (e: Throwable) {
            Log.w(TAG, "decode failed", e)
            closeStream()
            reply(AsrProtocol.ERROR, "The speech model stopped")
        }
    }

    private fun finish() {
        val s = stream ?: return
        try {
            reply(AsrProtocol.FINAL, normalizeTranscript(s.finish()))
        } catch (e: Throwable) {
            Log.w(TAG, "final decode failed", e)
            reply(AsrProtocol.ERROR, "The speech model stopped")
        } finally {
            closeStream()
        }
    }

    private fun closeStream() {
        stream?.close()
        stream = null
    }

    private fun reply(what: Int, text: String) {
        val m = Message.obtain(null, what, session, 0)
        m.data = Bundle().apply { putString(AsrProtocol.TEXT, text) }
        try {
            client?.send(m)
        } catch (_: RemoteException) {
            // The keyboard went away; it starts over on its next dictation.
            closeStream()
        }
    }

    private companion object {
        const val TAG = "SaysoAsr"
    }
}
