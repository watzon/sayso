package app.sayso.mobile.asr

import app.sayso.mobile.voice.Pcm
import com.k2fsa.sherpa.onnx.FeatureConfig
import com.k2fsa.sherpa.onnx.OfflineModelConfig
import com.k2fsa.sherpa.onnx.OfflineRecognizer
import com.k2fsa.sherpa.onnx.OfflineRecognizerConfig
import com.k2fsa.sherpa.onnx.OfflineTransducerModelConfig

fun createLocalAsr(): LocalAsr? = SherpaLocalAsr()

const val LOCAL_ASR_BUILT = true

/** sherpa-onnx offline transducers from files on disk, on the CPU. */
private class SherpaLocalAsr : LocalAsr {
    private var loaded: Pair<ModelOnDisk, OfflineRecognizer>? = null

    private val threads = Runtime.getRuntime().availableProcessors().coerceIn(1, 4)

    override fun open(model: ModelOnDisk): AsrStream {
        // Free a model that is no longer wanted before loading another.
        if (loaded != null && loaded?.first != model) release()
        val recognizer = loaded?.second ?: load(model).also { loaded = model to it }
        return Stream(recognizer)
    }

    override fun release() {
        loaded?.second?.release()
        loaded = null
    }

    private fun load(m: ModelOnDisk): OfflineRecognizer = OfflineRecognizer(
        config = OfflineRecognizerConfig(
            featConfig = FeatureConfig(sampleRate = Pcm.SAMPLE_RATE, featureDim = 80),
            modelConfig = OfflineModelConfig(
                transducer = OfflineTransducerModelConfig(
                    encoder = m.path(m.model.layout.encoder),
                    decoder = m.path(m.model.layout.decoder),
                    joiner = m.path(m.model.layout.joiner),
                ),
                tokens = m.path(m.model.layout.tokens),
                modelType = m.model.layout.modelType,
                numThreads = threads,
            ),
        ),
    )

    private class Stream(private val recognizer: OfflineRecognizer) : AsrStream {
        private var buffer = FloatArray(Pcm.SAMPLE_RATE * 10)
        private var size = 0

        override fun accept(samples: FloatArray) {
            if (size + samples.size > buffer.size) buffer = buffer.copyOf(maxOf(size + samples.size, buffer.size * 2))
            System.arraycopy(samples, 0, buffer, size, samples.size)
            size += samples.size
        }

        override fun finish(): String {
            val s = recognizer.createStream()
            try {
                s.acceptWaveform(buffer.copyOf(size), Pcm.SAMPLE_RATE)
                recognizer.decode(s)
                return recognizer.getResult(s).text
            } finally {
                s.release()
            }
        }

        override fun close() {
            buffer = FloatArray(0)
        }
    }
}
