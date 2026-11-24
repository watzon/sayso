package app.sayso.mobile.asr

import java.io.File

/** An installed model and its folder. */
data class ModelOnDisk(val model: AsrModel, val dir: File) {
    fun path(name: String): String = File(dir, name).absolutePath
}

/**
 * Local speech recognition. Runs only in the :asr process ([AsrService]), so
 * a native crash or an out-of-memory kill there never takes the keyboard down.
 * [createLocalAsr] is defined in src/sherpa (sherpa-onnx) or src/nosherpa (none).
 */
interface LocalAsr {
    /**
     * Starts one utterance with [model], which decodes the whole recording at
     * the end. Loads the model on first use and keeps it loaded for the next
     * call with the same model. Throws when the model cannot load.
     */
    fun open(model: ModelOnDisk): AsrStream

    /** Frees the loaded model. */
    fun release()
}

interface AsrStream {
    /** 16 kHz mono samples in [-1, 1]. */
    fun accept(samples: FloatArray)

    /** Ends the utterance and returns its text. */
    fun finish(): String

    fun close()
}
