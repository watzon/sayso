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
 * Skipped otherwise. See the README.
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

    private fun feed(stream: AsrStream, samples: FloatArray): List<String> {
        val partials = mutableListOf<String>()
        for (start in samples.indices step 640) {
            stream.accept(samples.copyOfRange(start, minOf(samples.size, start + 640)))
            stream.partial()?.let(partials::add)
        }
        return partials
    }

    @Test
    fun streamingModelGivesLiveAndFinalText() {
        val live = model(ModelCatalog.ZIPFORMER_EN)
        assumeTrue("needs -PsaysoAsrTestDir with the streaming model", live != null && File(dir, "0.wav").isFile)
        val asr = createLocalAsr()!!
        val stream = asr.open(final = null, live = live!!)
        val partials = feed(stream, sample())
        val text = stream.finish()
        stream.close()
        asr.release()
        println("live: ${partials.last()}\nfinal: $text")
        assertTrue("partials grow while speaking", partials.distinct().size > 3)
        assertEquals("AFTER EARLY NIGHTFALL THE YELLOW LAMPS WOULD LIGHT UP HERE AND THERE THE SQUALID QUARTER OF THE BROTHELS", text.trim())
    }

    @Test
    fun parakeetGivesTheFinalTextWithLiveTextFromTheStreamingModel() {
        val live = model(ModelCatalog.ZIPFORMER_EN)
        val final = model(ModelCatalog.PARAKEET_V3)
        assumeTrue("needs -PsaysoAsrTestDir with both models", live != null && final != null && File(dir, "0.wav").isFile)
        val asr = createLocalAsr()!!
        val stream = asr.open(final = final, live = live!!)
        val partials = feed(stream, sample())
        val text = stream.finish()
        stream.close()
        asr.release()
        println("live: ${partials.last()}\nfinal: $text")
        assertTrue(partials.last().isNotEmpty())
        val words = text.lowercase().replace(Regex("[^a-z ]"), "")
        assertTrue(text, words.contains("after early nightfall the yellow lamps would light up here and there"))
        assertTrue("Parakeet writes punctuation and case", text.any { it.isUpperCase() } && text.any { it.isLowerCase() })
    }
}
