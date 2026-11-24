package app.sayso.mobile.asr

/** One file of a model, checked by size and sha256 after download. */
data class ModelFile(val name: String, val url: String, val sizeBytes: Long, val sha256: String)

/**
 * The files of a sherpa-onnx transducer model, by role. [modelType] is
 * sherpa-onnx's own name for the architecture.
 */
data class TransducerLayout(
    val encoder: String,
    val decoder: String,
    val joiner: String,
    val tokens: String,
    val modelType: String,
)

data class AsrModel(
    val id: String,
    val name: String,
    /** BCP 47 language tags the model was trained on. */
    val languages: List<String>,
    val license: String,
    val files: List<ModelFile>,
    val layout: TransducerLayout,
    /**
     * Device RAM below which the model may be killed or slow, or 0. Compared
     * against ActivityManager.MemoryInfo.totalMem, which reports less than the
     * RAM on the box (see [ModelManager.ramAdvisory]).
     */
    val recommendedRamBytes: Long,
) {
    val downloadBytes: Long get() = files.sumOf { it.sizeBytes }
}

/**
 * The models the keyboard can download. They are never in the APK. Each one
 * decodes the whole recording when the dictation ends.
 *
 * Files come from the sherpa-onnx project's Hugging Face repositories, pinned to
 * a commit so their content cannot change under the hashes. The Parakeet files
 * are byte for byte the ones in the sherpa-onnx GitHub release archive
 * sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2 (asr-models release,
 * sha256 1495e91b…14b0f9). Single files are used instead of the archives so
 * the app needs no bzip2 decoder, a download resumes per file, and unused
 * fp32 weights and test audio are not fetched.
 */
object ModelCatalog {
    private fun hf(repo: String, revision: String, name: String, size: Long, sha256: String) =
        ModelFile(name, "https://huggingface.co/csukuangfj/$repo/resolve/$revision/$name", size, sha256)

    private const val PARAKEET_REPO = "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8"
    private const val PARAKEET_REV = "ed5d745623472f97cfc2e6c2916ee248798fc5a8"

    val PARAKEET_V3 = AsrModel(
        id = "parakeet-tdt-0.6b-v3-int8",
        name = "Parakeet TDT 0.6B v3",
        languages = listOf(
            "bg", "cs", "da", "de", "el", "en", "es", "et", "fi", "fr", "hr", "hu", "it",
            "lt", "lv", "mt", "nl", "pl", "pt", "ro", "ru", "sk", "sl", "sv", "uk",
        ),
        license = "CC-BY-4.0 (NVIDIA)",
        files = listOf(
            hf(PARAKEET_REPO, PARAKEET_REV, "encoder.int8.onnx", 652_282_294, "1afbe69c1ffdabee94755f42e4daf1d93549131a3da11356bda84fcef0f2c479"),
            hf(PARAKEET_REPO, PARAKEET_REV, "decoder.int8.onnx", 11_845_274, "0222a86147299d4dee90edf95b6d13827edb42ad8b7c9cf0b2582f030937c0b5"),
            hf(PARAKEET_REPO, PARAKEET_REV, "joiner.int8.onnx", 6_355_277, "cbd02f3696e9944970111fb439d9c3fd365a80374565f13f2b9c7007d253fc98"),
            hf(PARAKEET_REPO, PARAKEET_REV, "tokens.txt", 93_939, "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d"),
        ),
        layout = TransducerLayout("encoder.int8.onnx", "decoder.int8.onnx", "joiner.int8.onnx", "tokens.txt", "nemo_transducer"),
        recommendedRamBytes = 6L shl 30,
    )

    val ALL: List<AsrModel> = listOf(PARAKEET_V3)

    fun byId(id: String?): AsrModel? = ALL.firstOrNull { it.id == id }
}
