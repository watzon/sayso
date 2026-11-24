package app.sayso.mobile.asr

import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.ServiceConnection
import android.os.Bundle
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.Message
import android.os.Messenger
import android.os.RemoteException

/**
 * The keyboard's side of [AsrService]. Binds on the first dictation, queues
 * messages until the service is connected, and unbinds after [IDLE_UNBIND_MS]
 * without a dictation, which frees the model's memory.
 *
 * If the :asr process dies (a native crash, or the system killed it for
 * memory) the listener gets [Listener.onAsrError] with processDied = true,
 * once, for the session that was running; the next [start] binds again.
 * All calls and callbacks are on the main thread.
 */
class AsrClient(context: Context, private val listener: Listener) {
    interface Listener {
        fun onAsrFinal(session: Int, text: String)
        fun onAsrError(session: Int, message: String, processDied: Boolean)
    }

    private val app = context.applicationContext
    private val main = Handler(Looper.getMainLooper())
    private var service: Messenger? = null
    private var bound = false
    private val pending = ArrayList<Message>()
    private var session = 0

    private val incoming = Messenger(Handler(Looper.getMainLooper()) { msg ->
        val text = msg.data?.getString(AsrProtocol.TEXT).orEmpty()
        when (msg.what) {
            AsrProtocol.FINAL -> listener.onAsrFinal(msg.arg1, text)
            AsrProtocol.ERROR -> listener.onAsrError(msg.arg1, text, processDied = false)
        }
        true
    })

    private val death = IBinder.DeathRecipient { main.post(::onDied) }

    private val connection = object : ServiceConnection {
        override fun onServiceConnected(name: ComponentName?, binder: IBinder) {
            service = Messenger(binder)
            try {
                binder.linkToDeath(death, 0)
            } catch (_: RemoteException) {
                onDied()
                return
            }
            val queued = pending.toList()
            pending.clear()
            queued.forEach(::send)
        }

        override fun onServiceDisconnected(name: ComponentName?) = onDied()
        override fun onBindingDied(name: ComponentName?) = onDied()
        override fun onNullBinding(name: ComponentName?) = onDied()
    }

    private val idleUnbind = Runnable { unbind() }

    fun start(session: Int, model: ModelOnDisk) {
        main.removeCallbacks(idleUnbind)
        this.session = session
        if (!bound) {
            bound = app.bindService(Intent(app, AsrService::class.java), connection, Context.BIND_AUTO_CREATE)
            if (!bound) {
                listener.onAsrError(session, "The speech engine did not start", processDied = true)
                return
            }
        }
        send(message(AsrProtocol.START, session).apply { data = AsrProtocol.startData(model) })
    }

    fun audio(session: Int, pcm: ShortArray) {
        if (session != this.session) return
        send(message(AsrProtocol.AUDIO, session).apply { data = Bundle().apply { putShortArray(AsrProtocol.PCM, pcm) } })
    }

    fun finish(session: Int) {
        if (session == this.session) send(message(AsrProtocol.FINISH, session))
    }

    /** Drops the session and starts the idle timer. */
    fun cancel(session: Int) {
        if (session != this.session) return
        send(message(AsrProtocol.CANCEL, session))
        idle()
    }

    /** Drops whatever session is running, if any. */
    fun cancelCurrent() {
        if (session != 0) cancel(session)
    }

    /** The dictation is over: unbind after a while unless another one starts. */
    fun idle() {
        session = 0
        main.removeCallbacks(idleUnbind)
        main.postDelayed(idleUnbind, IDLE_UNBIND_MS)
    }

    fun release() {
        main.removeCallbacks(idleUnbind)
        unbind()
    }

    private fun message(what: Int, session: Int) = Message.obtain(null, what, session, 0).also { it.replyTo = incoming }

    private fun send(msg: Message) {
        val s = service
        if (s == null) {
            if (bound) pending += msg
            return
        }
        try {
            s.send(msg)
        } catch (_: RemoteException) {
            onDied()
        }
    }

    private fun onDied() {
        if (!bound && service == null) return
        val dead = session
        unbind()
        if (dead != 0) listener.onAsrError(dead, "The on-device engine stopped", processDied = true)
    }

    private fun unbind() {
        session = 0
        pending.clear()
        service?.binder?.let { runCatching { it.unlinkToDeath(death, 0) } }
        service = null
        if (bound) {
            bound = false
            runCatching { app.unbindService(connection) }
        }
    }

    companion object {
        const val IDLE_UNBIND_MS = 60_000L
    }
}
