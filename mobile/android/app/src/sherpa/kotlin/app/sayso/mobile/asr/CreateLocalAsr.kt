package app.sayso.mobile.asr

import app.sayso.mobile.voice.Pcm
import com.k2fsa.sherpa.onnx.FeatureConfig
import com.k2fsa.sherpa.onnx.OfflineModelConfig
import com.k2fsa.sherpa.onnx.OfflineRecognizer
import com.k2fsa.sherpa.onnx.OfflineRecognizerConfig
import com.k2fsa.sherpa.onnx.OfflineTransducerModelConfig
import com.k2fsa.sherpa.onnx.OnlineModelConfig
import com.k2fsa.sherpa.onnx.OnlineRecognizer
import com.k2fsa.sherpa.onnx.OnlineRecognizerConfig
import com.k2fsa.sherpa.onnx.OnlineStream
import com.k2fsa.sherpa.onnx.OnlineTransducerModelConfig

fun createLocalAsr(): LocalAsr? = SherpaLocalAsr()

const val LOCAL_ASR_BUILT = true

/** sherpa-onnx transducers from files on disk, on the CPU. */
private class SherpaLocalAsr : LocalAsr {
    private var online: Pair<ModelOnDisk, OnlineRecognizer>? = null
    private var offline: Pair<ModelOnDisk, OfflineRecognizer>? = null

    private val threads = Runtime.getRuntime().availableProcessors().coerceIn(1, 4)

    override fun open(final: ModelOnDisk?, live: ModelOnDisk?): AsrStream {
        require(final != null || live != null) { "no model" }
        // Free a model that is no longer wanted before loading another.
        if (online != null && online?.first != live) {
            online?.second?.release()
            online = null
        }
        if (offline != null && offline?.first != final) {
            offline?.second?.release()
            offline = null
        }
        val on = live?.let { m -> online?.second ?: loadOnline(m).also { online = m to it } }
        val off = final?.let { m -> offline?.second ?: loadOffline(m).also { offline = m to it } }
        return Stream(on, off)
    }

    override fun release() {
        online?.second?.release()
        offline?.second?.release()
        online = null
        offline = null
    }

    private fun loadOnline(m: ModelOnDisk): OnlineRecognizer = OnlineRecognizer(
        config = OnlineRecognizerConfig(
            featConfig = FeatureConfig(sampleRate = Pcm.SAMPLE_RATE, featureDim = 80),
            modelConfig = OnlineModelConfig(
                transducer = OnlineTransducerModelConfig(
                    encoder = m.path(m.model.layout.encoder),
                    decoder = m.path(m.model.layout.decoder),
                    joiner = m.path(m.model.layout.joiner),
                ),
                tokens = m.path(m.model.layout.tokens),
                modelType = m.model.layout.modelType,
                numThreads = 2,
            ),
            // The user ends the dictation; no endpoint detection.
            enableEndpoint = false,
        ),
    )

    private fun loadOffline(m: ModelOnDisk): OfflineRecognizer = OfflineRecognizer(
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

    private class Stream(private val on: OnlineRecognizer?, private val off: OfflineRecognizer?) : AsrStream {
        private val onStream: OnlineStream? = on?.createStream()
        private var buffer = FloatArray(if (off != null) Pcm.SAMPLE_RATE * 10 else 0)
        private var size = 0

        override fun accept(samples: FloatArray) {
            if (on != null && onStream != null) {
                onStream.acceptWaveform(samples, Pcm.SAMPLE_RATE)
                while (on.isReady(onStream)) on.decode(onStream)
            }
            if (off != null) {
                if (size + samples.size > buffer.size) buffer = buffer.copyOf(maxOf(size + samples.size, buffer.size * 2))
                System.arraycopy(samples, 0, buffer, size, samples.size)
                size += samples.size
            }
        }

        override fun partial(): String? = if (on != null && onStream != null) on.getResult(onStream).text else null

        override fun finish(): String {
            if (off != null) {
                val s = off.createStream()
                try {
                    s.acceptWaveform(buffer.copyOf(size), Pcm.SAMPLE_RATE)
                    off.decode(s)
                    return off.getResult(s).text
                } finally {
                    s.release()
                }
            }
            val o = on!!
            val st = onStream!!
            // Trailing silence flushes the last frames out of the streaming encoder.
            st.acceptWaveform(FloatArray(Pcm.SAMPLE_RATE * 2 / 5), Pcm.SAMPLE_RATE)
            st.inputFinished()
            while (o.isReady(st)) o.decode(st)
            return o.getResult(st).text
        }

        override fun close() {
            onStream?.release()
            buffer = FloatArray(0)
        }
    }
}
