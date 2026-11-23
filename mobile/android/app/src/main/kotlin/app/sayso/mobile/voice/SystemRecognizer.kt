package app.sayso.mobile.voice

import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.Bundle
import android.speech.RecognitionListener
import android.speech.RecognizerIntent
import android.speech.SpeechRecognizer

/**
 * Android's SpeechRecognizer: works with no download. The on-device
 * recognizer is used where the device has one (Android 12+), else the
 * default service. SpeechRecognizer records the microphone itself, so the
 * keyboard's own AudioCapture must be stopped before [start].
 *
 * One instance per dictation; every callback is on the main thread.
 */
class SystemRecognizer(
    private val context: Context,
    private val onLevel: (Float) -> Unit,
    private val onPartial: (String) -> Unit,
    private val onFinal: (String) -> Unit,
    private val onError: (String) -> Unit,
) {
    private var recognizer: SpeechRecognizer? = null
    private var onDevice = false
    private var preferOffline = false
    private var done = false

    /** [sensitive]: ask the service to stay on the device. */
    fun start(sensitive: Boolean) {
        preferOffline = sensitive
        onDevice = Build.VERSION.SDK_INT >= Build.VERSION_CODES.S && SpeechRecognizer.isOnDeviceRecognitionAvailable(context)
        listen()
    }

    private fun listen() {
        recognizer?.destroy()
        val r = if (onDevice && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            SpeechRecognizer.createOnDeviceSpeechRecognizer(context)
        } else {
            SpeechRecognizer.createSpeechRecognizer(context)
        }
        recognizer = r
        r.setRecognitionListener(listener)
        r.startListening(
            Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH).apply {
                putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL, RecognizerIntent.LANGUAGE_MODEL_FREE_FORM)
                putExtra(RecognizerIntent.EXTRA_PARTIAL_RESULTS, true)
                putExtra(RecognizerIntent.EXTRA_MAX_RESULTS, 1)
                putExtra(RecognizerIntent.EXTRA_CALLING_PACKAGE, context.packageName)
                if (preferOffline) putExtra(RecognizerIntent.EXTRA_PREFER_OFFLINE, true)
            },
        )
    }

    /** Stop listening; the final text follows through onFinal. */
    fun stop() {
        recognizer?.stopListening()
    }

    fun cancel() {
        done = true
        recognizer?.cancel()
        recognizer?.destroy()
        recognizer = null
    }

    private val listener = object : RecognitionListener {
        override fun onReadyForSpeech(params: Bundle?) {}
        override fun onBeginningOfSpeech() {}
        override fun onRmsChanged(rmsdB: Float) {
            if (!done) onLevel(Pcm.systemRmsLevel(rmsdB))
        }
        override fun onBufferReceived(buffer: ByteArray?) {}
        override fun onEndOfSpeech() {}
        override fun onEvent(eventType: Int, params: Bundle?) {}

        override fun onPartialResults(partialResults: Bundle?) {
            if (!done) firstResult(partialResults)?.let(onPartial)
        }

        override fun onResults(results: Bundle?) {
            if (done) return
            done = true
            onFinal(firstResult(results).orEmpty())
            cancel()
        }

        override fun onError(error: Int) {
            if (done) return
            // An on-device recognizer without this language: try the default service once.
            if (onDevice && error in LANGUAGE_ERRORS) {
                onDevice = false
                listen()
                return
            }
            done = true
            onError(messageFor(error))
            cancel()
        }
    }

    private fun firstResult(b: Bundle?): String? =
        b?.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)?.firstOrNull()

    companion object {
        // ERROR_LANGUAGE_NOT_SUPPORTED and ERROR_LANGUAGE_UNAVAILABLE (API 31).
        private val LANGUAGE_ERRORS = setOf(12, 13)

        fun available(context: Context): Boolean = SpeechRecognizer.isRecognitionAvailable(context)

        fun messageFor(error: Int): String = when (error) {
            SpeechRecognizer.ERROR_NO_MATCH, SpeechRecognizer.ERROR_SPEECH_TIMEOUT -> "Didn't catch that"
            SpeechRecognizer.ERROR_INSUFFICIENT_PERMISSIONS -> "Microphone access is off"
            SpeechRecognizer.ERROR_RECOGNIZER_BUSY -> "The system recognizer is busy"
            SpeechRecognizer.ERROR_NETWORK, SpeechRecognizer.ERROR_NETWORK_TIMEOUT, SpeechRecognizer.ERROR_SERVER ->
                "The system recognizer needs a connection"
            SpeechRecognizer.ERROR_AUDIO -> "The microphone is in use"
            else -> "The system recognizer stopped (error $error)"
        }
    }
}
