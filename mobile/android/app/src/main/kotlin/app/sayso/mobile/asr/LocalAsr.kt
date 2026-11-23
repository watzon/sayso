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
     * Starts one utterance. Loads the models on first use and keeps them loaded
     * for the next call with the same models. Throws when a model cannot load.
     * [final] decodes the whole recording at the end; [live] decodes while you
     * speak and gives the final text when [final] is null. At least one is set.
     */
    fun open(final: ModelOnDisk?, live: ModelOnDisk?): AsrStream

    /** Frees every loaded model. */
    fun release()
}

interface AsrStream {
    /** 16 kHz mono samples in [-1, 1]. */
    fun accept(samples: FloatArray)

    /** The live text so far, or null when there is no live model. */
    fun partial(): String?

    /** Ends the utterance and returns its text. */
    fun finish(): String

    fun close()
}
