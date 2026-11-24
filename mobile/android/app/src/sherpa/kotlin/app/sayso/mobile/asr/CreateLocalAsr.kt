package app.sayso.mobile.asr

import app.sayso.mobile.voice.Pcm
import com.k2fsa.sherpa.onnx.FeatureConfig
import com.k2fsa.sherpa.onnx.OfflineModelConfig
import com.k2fsa.sherpa.onnx.OfflineMoonshineModelConfig
import com.k2fsa.sherpa.onnx.OfflineRecognizer
import com.k2fsa.sherpa.onnx.OfflineRecognizerConfig
import com.k2fsa.sherpa.onnx.OfflineTransducerModelConfig
import com.k2fsa.sherpa.onnx.OfflineWhisperModelConfig

fun createLocalAsr(): LocalAsr? = SherpaLocalAsr()

const val LOCAL_ASR_BUILT = true

/** sherpa-onnx offline models from files on disk, on the CPU. */
private class SherpaLocalAsr : LocalAsr {
    private var loaded: Pair<ModelOnDisk, OfflineRecognizer>? = null

    private val threads = Runtime.getRuntime().availableProcessors().coerceIn(1, 4)

    override fun open(model: ModelOnDisk): AsrStream {
        // Free a model that is no longer wanted before loading another.
        if (loaded != null && loaded?.first != model) release()
        val recognizer = loaded?.second ?: load(model).also { loaded = model to it }
        return Stream(recognizer, model.model.layout.maxSegmentSeconds * Pcm.SAMPLE_RATE)
    }

    override fun release() {
        loaded?.second?.release()
        loaded = null
    }

    private fun load(m: ModelOnDisk): OfflineRecognizer = OfflineRecognizer(
        config = OfflineRecognizerConfig(
            featConfig = FeatureConfig(sampleRate = Pcm.SAMPLE_RATE, featureDim = 80),
            modelConfig = offlineModelConfig(m, threads),
        ),
    )

    /** [maxSegment] in samples, or 0 to decode the whole recording at once. */
    private class Stream(private val recognizer: OfflineRecognizer, private val maxSegment: Int) : AsrStream {
        private var buffer = FloatArray(Pcm.SAMPLE_RATE * 10)
        private var size = 0

        override fun accept(samples: FloatArray) {
            if (size + samples.size > buffer.size) buffer = buffer.copyOf(maxOf(size + samples.size, buffer.size * 2))
            System.arraycopy(samples, 0, buffer, size, samples.size)
            size += samples.size
        }

        override fun finish(): String {
            val samples = buffer.copyOf(size)
            if (maxSegment <= 0 || size <= maxSegment) return decode(samples)
            return splitAtPauses(samples, maxSegment, search = SEARCH, frame = FRAME)
                .map { decode(samples.copyOfRange(it.first, it.last + 1)).trim() }
                .filter { it.isNotEmpty() }
                .joinToString(" ")
        }

        private fun decode(samples: FloatArray): String {
            val s = recognizer.createStream()
            try {
                s.acceptWaveform(samples, Pcm.SAMPLE_RATE)
                recognizer.decode(s)
                return recognizer.getResult(s).text
            } finally {
                s.release()
            }
        }

        override fun close() {
            buffer = FloatArray(0)
        }

        private companion object {
            /** Cuts are looked for in the last 5 seconds before the limit, in 30 ms frames. */
            const val SEARCH = Pcm.SAMPLE_RATE * 5
            const val FRAME = Pcm.SAMPLE_RATE * 3 / 100
        }
    }
}

/**
 * The sherpa-onnx 2.0.3 config for [m]. Whisper's modelType is "whisper" and
 * Moonshine's is empty, as in sherpa-onnx's own Android examples.
 */
internal fun offlineModelConfig(m: ModelOnDisk, threads: Int): OfflineModelConfig {
    val base = OfflineModelConfig(tokens = m.path(m.model.layout.tokens), numThreads = threads)
    return when (val l = m.model.layout) {
        is TransducerLayout -> base.copy(
            transducer = OfflineTransducerModelConfig(encoder = m.path(l.encoder), decoder = m.path(l.decoder), joiner = m.path(l.joiner)),
            modelType = l.modelType,
        )
        is WhisperLayout -> base.copy(
            whisper = OfflineWhisperModelConfig(
                encoder = m.path(l.encoder),
                decoder = m.path(l.decoder),
                language = l.language,
                task = l.task,
                tailPaddings = l.tailPaddings,
            ),
            modelType = "whisper",
        )
        is MoonshineLayout -> base.copy(
            moonshine = OfflineMoonshineModelConfig(
                preprocessor = m.path(l.preprocessor),
                encoder = m.path(l.encoder),
                uncachedDecoder = m.path(l.uncachedDecoder),
                cachedDecoder = m.path(l.cachedDecoder),
            ),
        )
    }
}
