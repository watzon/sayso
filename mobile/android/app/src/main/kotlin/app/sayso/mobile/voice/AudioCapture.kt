package app.sayso.mobile.voice

import android.annotation.SuppressLint
import android.content.Context
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioRecord
import android.media.MediaRecorder
import android.os.Handler
import android.os.Looper
import android.util.Log

/**
 * The microphone at 16 kHz mono PCM16, read on its own thread in 40 ms
 * chunks. [onChunk] and [onError] run on that thread.
 *
 * The keyboard may record while it is on screen: the system binds a visible
 * IME with BIND_INCLUDE_CAPABILITIES, so it shares the foreground app's
 * while-in-use microphone access. RECORD_AUDIO must be granted (the
 * companion app asks for it).
 */
class AudioCapture(
    private val onChunk: (pcm: ShortArray, level: Float) -> Unit,
    private val onError: () -> Unit,
) {
    @Volatile private var running = false
    private var record: AudioRecord? = null
    private var thread: Thread? = null

    /** False when the microphone could not open (no permission, or another app holds it). */
    @SuppressLint("MissingPermission") // the caller checks RECORD_AUDIO first
    fun start(): Boolean {
        if (running) return true
        val min = AudioRecord.getMinBufferSize(Pcm.SAMPLE_RATE, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
        if (min <= 0) return false
        val r = try {
            AudioRecord(
                MediaRecorder.AudioSource.VOICE_RECOGNITION,
                Pcm.SAMPLE_RATE,
                AudioFormat.CHANNEL_IN_MONO,
                AudioFormat.ENCODING_PCM_16BIT,
                maxOf(min, Pcm.SAMPLE_RATE), // half a second of samples
            )
        } catch (e: RuntimeException) {
            Log.w(TAG, "AudioRecord", e)
            return false
        }
        if (r.state != AudioRecord.STATE_INITIALIZED) {
            r.release()
            return false
        }
        try {
            r.startRecording()
        } catch (e: IllegalStateException) {
            r.release()
            return false
        }
        if (r.recordingState != AudioRecord.RECORDSTATE_RECORDING) {
            r.release()
            return false
        }
        record = r
        running = true
        thread = Thread({ loop(r) }, "sayso-mic").apply { start() }
        return true
    }

    private fun loop(r: AudioRecord) {
        val buf = ShortArray(CHUNK)
        while (running) {
            val n = r.read(buf, 0, CHUNK)
            if (n < 0) {
                if (running) onError()
                return
            }
            if (n > 0 && running) onChunk(buf.copyOf(n), Pcm.level(buf, n))
        }
    }

    /** Stops and releases the microphone before it returns, so another recorder can take it at once. */
    fun stop() {
        if (!running && record == null) return
        running = false
        val r = record
        record = null
        runCatching { r?.stop() } // unblocks read()
        thread?.join(500)
        thread = null
        r?.release()
    }

    private companion object {
        const val TAG = "SaysoMic"
        const val CHUNK = Pcm.SAMPLE_RATE / 25 // 40 ms
    }
}

/**
 * Transient exclusive audio focus while dictating, so music and videos pause
 * and resume afterwards. [onLoss] runs when something else takes focus, such
 * as a phone call.
 */
class DictationAudioFocus(context: Context, private val onLoss: () -> Unit) {
    private val audio = context.getSystemService(AudioManager::class.java)
    private val request = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN_TRANSIENT_EXCLUSIVE)
        .setAudioAttributes(
            AudioAttributes.Builder()
                .setUsage(AudioAttributes.USAGE_ASSISTANT)
                .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
                .build(),
        )
        .setOnAudioFocusChangeListener({ change ->
            if (held && change < 0) onLoss()
        }, Handler(Looper.getMainLooper()))
        .build()
    private var held = false

    fun request() {
        if (held) return
        held = audio.requestAudioFocus(request) == AudioManager.AUDIOFOCUS_REQUEST_GRANTED
    }

    fun abandon() {
        if (!held) return
        held = false
        audio.abandonAudioFocusRequest(request)
    }
}
