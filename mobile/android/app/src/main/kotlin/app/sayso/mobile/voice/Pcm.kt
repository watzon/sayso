package app.sayso.mobile.voice

import kotlin.math.log10
import kotlin.math.sqrt

/** 16 kHz mono PCM16, the format of every engine. */
object Pcm {
    const val SAMPLE_RATE = 16_000

    /** Loudness of a chunk from 0 (at or below -55 dBFS) to 1 (-10 dBFS or louder), for the waveform. */
    fun level(samples: ShortArray, count: Int = samples.size): Float {
        if (count <= 0) return 0f
        var sum = 0.0
        for (i in 0 until count) {
            val v = samples[i] / 32768.0
            sum += v * v
        }
        val rms = sqrt(sum / count)
        if (rms <= 0.0) return 0f
        return dbToLevel(20 * log10(rms), floorDb = -55.0, ceilDb = -10.0)
    }

    /** SpeechRecognizer's onRmsChanged gives about -2 dB in silence to 10 dB for loud speech. */
    fun systemRmsLevel(rmsDb: Float): Float = dbToLevel(rmsDb.toDouble(), floorDb = -2.0, ceilDb = 10.0)

    private fun dbToLevel(db: Double, floorDb: Double, ceilDb: Double): Float =
        ((db - floorDb) / (ceilDb - floorDb)).coerceIn(0.0, 1.0).toFloat()

    fun toFloats(samples: ShortArray, count: Int = samples.size): FloatArray =
        FloatArray(count) { samples[it] / 32768f }

    fun durationMs(samples: Int): Long = samples * 1000L / SAMPLE_RATE
}

/** The whole recording, for the cloud and for the batch model, up to [maxSamples]. */
class PcmBuffer(private val maxSamples: Int) {
    private var data = ShortArray(Pcm.SAMPLE_RATE)
    var size = 0
        private set

    /** Appends what fits. False when the buffer is full and samples were dropped. */
    fun append(chunk: ShortArray, count: Int = chunk.size): Boolean {
        val n = minOf(count, maxSamples - size)
        if (n > 0) {
            if (size + n > data.size) data = data.copyOf(minOf(maxSamples, maxOf(size + n, data.size * 2)))
            System.arraycopy(chunk, 0, data, size, n)
            size += n
        }
        return n == count
    }

    fun toShortArray(): ShortArray = data.copyOf(size)

    fun clear() {
        size = 0
    }
}

/**
 * Some models (the streaming Zipformer) write in capitals with no
 * punctuation. Lowercases such text, capitalizes the first letter, and
 * writes the pronoun "I" in capitals; VoiceCore.postProcess does the rest.
 * Text with any lowercase letter is returned as it is.
 */
fun normalizeTranscript(text: String): String {
    val t = text.trim().replace(Regex("\\s+"), " ")
    if (t.isEmpty() || t.none { it.isLetter() } || t.any { it.isLowerCase() }) return t
    val lower = t.lowercase().replace(Regex("\\bi\\b"), "I")
    return lower.replaceFirstChar { it.uppercaseChar() }
}
