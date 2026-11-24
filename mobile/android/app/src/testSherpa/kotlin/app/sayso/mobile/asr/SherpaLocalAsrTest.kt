package app.sayso.mobile.asr

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test
import java.io.File
import java.nio.ByteBuffer
import java.nio.ByteOrder

/**
 * Runs the real sherpa-onnx engine with the catalog's model layouts on this
 * computer. Opt-in: needs -PsaysoAsrTestDir=<dir> with lib/ (the sherpa-onnx
 * JNI library for this OS, from the same release as the AAR),
 * models/<model id>/ (the catalog files), and 0.wav (LibriSpeech
 * 1089-134686-0000, from the sherpa-onnx model repositories' test_wavs).
 * Each model's test is skipped when its files are missing. See the README.
 */
class SherpaLocalAsrTest {
    private val dir = System.getProperty("sayso.asrTestDir")?.let(::File)

    private fun model(m: AsrModel): ModelOnDisk? {
        val d = File(dir ?: return null, "models/${m.id}")
        return if (m.files.all { File(d, it.name).length() == it.sizeBytes }) ModelOnDisk(m, d) else null
    }

    /** 16 kHz mono PCM16 WAV. */
    private fun sample(): FloatArray {
        val bytes = File(dir!!, "0.wav").readBytes()
        var i = 12
        while (String(bytes, i, 4) != "data") i += 8 + ByteBuffer.wrap(bytes, i + 4, 4).order(ByteOrder.LITTLE_ENDIAN).int
        val start = i + 8
        val pcm = ByteBuffer.wrap(bytes, start, bytes.size - start).order(ByteOrder.LITTLE_ENDIAN).asShortBuffer()
        return FloatArray(pcm.remaining()) { pcm.get(it) / 32768f }
    }

    /** The sample [times] times with half a second of silence after each, about 7 seconds apiece. */
    private fun transcribe(m: AsrModel, times: Int = 1): String {
        val model = model(m)
        assumeTrue("needs -PsaysoAsrTestDir with ${m.id}", model != null && File(dir, "0.wav").isFile)
        val asr = createLocalAsr()!!
        val stream = asr.open(model!!)
        val one = sample()
        val samples = if (times == 1) one else FloatArray(0).let { var a = it; repeat(times) { a += one + FloatArray(8_000) }; a }
        for (start in samples.indices step 640) stream.accept(samples.copyOfRange(start, minOf(samples.size, start + 640)))
        val text = stream.finish()
        stream.close()
        asr.release()
        println("${m.id}: $text")
        return text
    }

    private fun assertHeardTheSample(text: String) {
        val words = text.lowercase().replace(Regex("[^a-z ]"), " ").replace(Regex(" +"), " ")
        assertTrue(text, words.contains("after early nightfall the yellow lamps would light up here and there"))
    }

    @Test
    fun parakeetGivesTheFinalText() {
        val text = transcribe(ModelCatalog.PARAKEET_V3)
        assertHeardTheSample(text)
        assertTrue("Parakeet writes punctuation and case", text.any { it.isUpperCase() } && text.any { it.isLowerCase() })
    }

    @Test
    fun parakeetUltraGivesTheFinalText() {
        val text = transcribe(ModelCatalog.PARAKEET_ULTRA)
        assertHeardTheSample(text)
        assertTrue("Parakeet writes punctuation and case", text.any { it.isUpperCase() } && text.any { it.isLowerCase() })
    }

    @Test
    fun moonshineTinyGivesTheFinalText() = assertHeardTheSample(transcribe(ModelCatalog.MOONSHINE_TINY))

    @Test
    fun whisperBaseGivesTheFinalText() = assertHeardTheSample(transcribe(ModelCatalog.WHISPER_BASE_EN))

    @Test
    fun whisperSmallGivesTheFinalText() = assertHeardTheSample(transcribe(ModelCatalog.WHISPER_SMALL_EN))

    private fun assertHeardEveryRepeat(m: AsrModel) {
        val text = transcribe(m, times = 6)
        assertEquals(text, 6, Regex("yellow lamps").findAll(text.lowercase()).count())
    }

    /** sherpa-onnx drops what Whisper hears after 30 seconds. */
    @Test
    fun whisperHearsAllOfA40SecondRecording() = assertHeardEveryRepeat(ModelCatalog.WHISPER_BASE_EN)

    /** Moonshine repeats itself on a 40-second recording in one pass. */
    @Test
    fun moonshineHearsAllOfA40SecondRecordingOnce() = assertHeardEveryRepeat(ModelCatalog.MOONSHINE_TINY)
}

/** The config each layout maps to. Builds plain config objects; loads no native code. */
class OfflineModelConfigTest {
    private val dir = File("/m")

    @Test
    fun transducerUsesItsFilesAndModelType() {
        val c = offlineModelConfig(ModelOnDisk(ModelCatalog.PARAKEET_ULTRA, dir), threads = 3)
        assertEquals("/m/encoder.int8.onnx", c.transducer.encoder)
        assertEquals("/m/decoder.int8.onnx", c.transducer.decoder)
        assertEquals("/m/joiner.int8.onnx", c.transducer.joiner)
        assertEquals("/m/tokens.txt", c.tokens)
        assertEquals("nemo_transducer", c.modelType)
        assertEquals(3, c.numThreads)
        assertEquals("", c.whisper.encoder)
        assertEquals("", c.moonshine.encoder)
    }

    @Test
    fun whisperUsesItsFilesLanguageAndTask() {
        val c = offlineModelConfig(ModelOnDisk(ModelCatalog.WHISPER_BASE_EN, dir), threads = 2)
        assertEquals("/m/base.en-encoder.int8.onnx", c.whisper.encoder)
        assertEquals("/m/base.en-decoder.int8.onnx", c.whisper.decoder)
        assertEquals("en", c.whisper.language)
        assertEquals("transcribe", c.whisper.task)
        assertEquals(-1, c.whisper.tailPaddings)
        assertEquals("/m/base.en-tokens.txt", c.tokens)
        assertEquals("whisper", c.modelType)
        assertEquals("", c.transducer.encoder)
    }

    @Test
    fun moonshineUsesItsFourNetworks() {
        val c = offlineModelConfig(ModelOnDisk(ModelCatalog.MOONSHINE_TINY, dir), threads = 2)
        assertEquals("/m/preprocess.onnx", c.moonshine.preprocessor)
        assertEquals("/m/encode.int8.onnx", c.moonshine.encoder)
        assertEquals("/m/uncached_decode.int8.onnx", c.moonshine.uncachedDecoder)
        assertEquals("/m/cached_decode.int8.onnx", c.moonshine.cachedDecoder)
        assertEquals("", c.moonshine.mergedDecoder)
        assertEquals("/m/tokens.txt", c.tokens)
        assertEquals("", c.modelType)
        assertEquals("", c.transducer.encoder)
        assertEquals("", c.whisper.encoder)
    }
}
