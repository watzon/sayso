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
    /** Keyboard to service. Data: the model id and folder, see [model]. */
    const val START = 1

    /** Data: [PCM], a ShortArray of 16 kHz mono samples. */
    const val AUDIO = 2
    const val FINISH = 3
    const val CANCEL = 4

    /** Service to keyboard. Data: [TEXT]. */
    const val FINAL = 11

    /** Data: [TEXT], the reason. */
    const val ERROR = 12

    const val PCM = "pcm"
    const val TEXT = "text"
    private const val MODEL_ID = "model_id"
    private const val MODEL_DIR = "model_dir"

    fun startData(model: ModelOnDisk) = Bundle().apply {
        putString(MODEL_ID, model.model.id)
        putString(MODEL_DIR, model.dir.absolutePath)
    }

    /** The model of a [START] message, or null when it is not in the catalog. */
    fun model(data: Bundle): ModelOnDisk? {
        val model = ModelCatalog.byId(data.getString(MODEL_ID)) ?: return null
        val dir = data.getString(MODEL_DIR) ?: return null
        return ModelOnDisk(model, File(dir))
    }
}

/**
 * Hosts local speech recognition in its own process (android:process=":asr").
 * The model loads on the first dictation and stays loaded while the keyboard keeps
 * the service bound. All work runs on one background thread, in message order.
 */
class AsrService : Service() {
    private lateinit var thread: HandlerThread
    private lateinit var messenger: Messenger
    private var asr: LocalAsr? = null
    private var stream: AsrStream? = null
    private var session = 0
    private var client: Messenger? = null

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
        val model = AsrProtocol.model(msg.data)
        try {
            val engine = asr ?: createLocalAsr()?.also { asr = it } ?: return reply(AsrProtocol.ERROR, "This build has no local speech engine")
            if (model == null) return reply(AsrProtocol.ERROR, "No local model is installed")
            stream = engine.open(model)
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
