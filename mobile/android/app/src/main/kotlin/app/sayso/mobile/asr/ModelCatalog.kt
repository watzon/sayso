package app.sayso.mobile.asr

/** One file of a model, checked by size and sha256 after download. */
data class ModelFile(val name: String, val url: String, val sizeBytes: Long, val sha256: String)

/** The files of a sherpa-onnx offline model, by role, for one architecture. */
sealed interface ModelLayout {
    val tokens: String

    /** Every file the layout names. */
    val paths: List<String>

    /**
     * The longest stretch of audio the model decodes well in one pass, or 0
     * for no limit. Longer recordings are cut at pauses ([splitAtPauses]).
     */
    val maxSegmentSeconds: Int
}

/** [modelType] is sherpa-onnx's own name for the architecture. */
data class TransducerLayout(
    val encoder: String,
    val decoder: String,
    val joiner: String,
    override val tokens: String,
    val modelType: String = "nemo_transducer",
) : ModelLayout {
    override val paths get() = listOf(encoder, decoder, joiner, tokens)
    override val maxSegmentSeconds get() = 0
}

/**
 * [tailPaddings] below 0 lets sherpa-onnx pick the padding for the model.
 * sherpa-onnx decodes only the first 30 seconds of a recording with Whisper.
 */
data class WhisperLayout(
    val encoder: String,
    val decoder: String,
    override val tokens: String,
    val language: String = "en",
    val task: String = "transcribe",
    val tailPaddings: Int = -1,
) : ModelLayout {
    override val paths get() = listOf(encoder, decoder, tokens)
    override val maxSegmentSeconds get() = 25
}

/** Moonshine v1: four networks. On a 40-second recording it starts repeating itself. */
data class MoonshineLayout(
    val preprocessor: String,
    val encoder: String,
    val uncachedDecoder: String,
    val cachedDecoder: String,
    override val tokens: String,
) : ModelLayout {
    override val paths get() = listOf(preprocessor, encoder, uncachedDecoder, cachedDecoder, tokens)
    override val maxSegmentSeconds get() = 25
}

data class AsrModel(
    val id: String,
    val name: String,
    /** A few words on what the model is good at, shown under its name before its languages. */
    val summary: String,
    /** BCP 47 language tags the model was trained on. */
    val languages: List<String>,
    val license: String,
    val files: List<ModelFile>,
    val layout: ModelLayout,
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
 * The models the keyboard can download, smallest first. They are never in
 * the APK. Each one decodes the whole recording when the dictation ends.
 *
 * Files come from the sherpa-onnx project's Hugging Face repositories, pinned to
 * a commit so their content cannot change under the hashes. The Parakeet v3
 * files are byte for byte the ones in the sherpa-onnx GitHub release archive
 * sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2 (asr-models release,
 * sha256 1495e91b…14b0f9). Single files are used instead of the archives so
 * the app needs no bzip2 decoder, a download resumes per file, and unused
 * fp32 weights and test audio are not fetched.
 */
object ModelCatalog {
    private fun hf(repo: String, revision: String, name: String, size: Long, sha256: String) =
        ModelFile(name, "https://huggingface.co/csukuangfj/$repo/resolve/$revision/$name", size, sha256)

    private val EUROPEAN_25 = listOf(
        "bg", "cs", "da", "de", "el", "en", "es", "et", "fi", "fr", "hr", "hu", "it",
        "lt", "lv", "mt", "nl", "pl", "pt", "ro", "ru", "sk", "sl", "sv", "uk",
    )

    private const val MOONSHINE_REPO = "sherpa-onnx-moonshine-tiny-en-int8"
    private const val MOONSHINE_REV = "bf2b762c076d8ea61e2af0b3851c9564fb77552e"

    val MOONSHINE_TINY = AsrModel(
        id = "moonshine-tiny-en-int8",
        name = "Moonshine Tiny",
        summary = "Fastest, compact",
        languages = listOf("en"),
        license = "MIT (Useful Sensors)",
        files = listOf(
            hf(MOONSHINE_REPO, MOONSHINE_REV, "preprocess.onnx", 6_800_738, "f33addce61a143460fe753b5ee5b7db255e5140b5b779c065b94f6c83ff0bf4e"),
            hf(MOONSHINE_REPO, MOONSHINE_REV, "encode.int8.onnx", 18_249_187, "8774dfba578de027ec6595c2c654a0836434489bc963a0db124a7f181f571acb"),
            hf(MOONSHINE_REPO, MOONSHINE_REV, "uncached_decode.int8.onnx", 53_216_096, "216737000dd5881a17aa043f6bbd286add33e4c3b0ae257153e2ec15438bdc41"),
            hf(MOONSHINE_REPO, MOONSHINE_REV, "cached_decode.int8.onnx", 45_264_830, "2aff28bba6a03d8dcf5c9feac45462629bae37317442299f28115ad09da773f6"),
            hf(MOONSHINE_REPO, MOONSHINE_REV, "tokens.txt", 436_688, "1165c2aeb9f72f457a83be2d459a09054f27490acd9b41bd43794dfd25e296ea"),
        ),
        layout = MoonshineLayout("preprocess.onnx", "encode.int8.onnx", "uncached_decode.int8.onnx", "cached_decode.int8.onnx", "tokens.txt"),
        recommendedRamBytes = 0,
    )

    private const val WHISPER_BASE_REPO = "sherpa-onnx-whisper-base.en"
    private const val WHISPER_BASE_REV = "59eea950fc76df2453efb57e6c0fd334548e8ffe"

    val WHISPER_BASE_EN = AsrModel(
        id = "whisper-base-en-int8",
        name = "Whisper Base",
        summary = "Fast Whisper",
        languages = listOf("en"),
        license = "MIT (OpenAI)",
        files = listOf(
            hf(WHISPER_BASE_REPO, WHISPER_BASE_REV, "base.en-encoder.int8.onnx", 29_120_534, "ef6b936f4c9b1d90a3b68634b60c4ed8576b26172b33c2535ec0e933c9edb823"),
            hf(WHISPER_BASE_REPO, WHISPER_BASE_REV, "base.en-decoder.int8.onnx", 130_669_978, "f7162ad6db2dbef16cfaeaa7f945b9d7dd9c1b8d472f6aca82f2273d185e4d41"),
            hf(WHISPER_BASE_REPO, WHISPER_BASE_REV, "base.en-tokens.txt", 835_554, "306cd27f03c1a714eca7108e03d66b7dc042abe8c258b44c199a7ed9838dd930"),
        ),
        layout = WhisperLayout("base.en-encoder.int8.onnx", "base.en-decoder.int8.onnx", "base.en-tokens.txt"),
        recommendedRamBytes = 0,
    )

    private const val WHISPER_SMALL_REPO = "sherpa-onnx-whisper-small.en"
    private const val WHISPER_SMALL_REV = "d9533f69affd85061aee349af7fea5cb2996dbbe"

    val WHISPER_SMALL_EN = AsrModel(
        id = "whisper-small-en-int8",
        name = "Whisper Small",
        summary = "Accurate Whisper",
        languages = listOf("en"),
        license = "MIT (OpenAI)",
        files = listOf(
            hf(WHISPER_SMALL_REPO, WHISPER_SMALL_REV, "small.en-encoder.int8.onnx", 112_442_483, "8bdac288f369aa94ee2194059238c465ed82ea9d47ee8fa4a8c0a891873e462f"),
            hf(WHISPER_SMALL_REPO, WHISPER_SMALL_REV, "small.en-decoder.int8.onnx", 262_223_042, "710ccf890e10f3faa15f51ec346081a2723c9f3adb6e4da81c6573a5a6f877fb"),
            hf(WHISPER_SMALL_REPO, WHISPER_SMALL_REV, "small.en-tokens.txt", 835_554, "306cd27f03c1a714eca7108e03d66b7dc042abe8c258b44c199a7ed9838dd930"),
        ),
        layout = WhisperLayout("small.en-encoder.int8.onnx", "small.en-decoder.int8.onnx", "small.en-tokens.txt"),
        recommendedRamBytes = 0,
    )

    private const val ULTRA_REPO = "sherpa-onnx-nemo-parakeet-tdt-0.6b-ultra-int8"
    private const val ULTRA_REV = "402e1190ce85b9793118ee223db21fb0d12f52db"

    val PARAKEET_ULTRA = AsrModel(
        id = "parakeet-tdt-0.6b-ultra-int8",
        name = "Parakeet TDT 0.6B Ultra",
        summary = "Best accuracy",
        languages = EUROPEAN_25,
        license = "CC-BY-4.0 (Moondream, NVIDIA)",
        files = listOf(
            hf(ULTRA_REPO, ULTRA_REV, "encoder.int8.onnx", 652_282_345, "b43072e1b721f18a3df03fcfeb763ff7c4debdf29843598a3aba2f703e2780a3"),
            hf(ULTRA_REPO, ULTRA_REV, "decoder.int8.onnx", 11_845_274, "1fab98fe6c12aded87d2da66272cc9e148d0a0044ce3850a12fe56302ec4a922"),
            hf(ULTRA_REPO, ULTRA_REV, "joiner.int8.onnx", 6_355_277, "8a71aaccfdba3d451775507a889afd2257d2e754463fd81095a8c8f290f924c2"),
            hf(ULTRA_REPO, ULTRA_REV, "tokens.txt", 93_939, "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d"),
        ),
        layout = TransducerLayout("encoder.int8.onnx", "decoder.int8.onnx", "joiner.int8.onnx", "tokens.txt"),
        recommendedRamBytes = 6L shl 30,
    )

    private const val PARAKEET_REPO = "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8"
    private const val PARAKEET_REV = "ed5d745623472f97cfc2e6c2916ee248798fc5a8"

    val PARAKEET_V3 = AsrModel(
        id = "parakeet-tdt-0.6b-v3-int8",
        name = "Parakeet TDT 0.6B v3",
        summary = "Multilingual",
        languages = EUROPEAN_25,
        license = "CC-BY-4.0 (NVIDIA)",
        files = listOf(
            hf(PARAKEET_REPO, PARAKEET_REV, "encoder.int8.onnx", 652_282_294, "1afbe69c1ffdabee94755f42e4daf1d93549131a3da11356bda84fcef0f2c479"),
            hf(PARAKEET_REPO, PARAKEET_REV, "decoder.int8.onnx", 11_845_274, "0222a86147299d4dee90edf95b6d13827edb42ad8b7c9cf0b2582f030937c0b5"),
            hf(PARAKEET_REPO, PARAKEET_REV, "joiner.int8.onnx", 6_355_277, "cbd02f3696e9944970111fb439d9c3fd365a80374565f13f2b9c7007d253fc98"),
            hf(PARAKEET_REPO, PARAKEET_REV, "tokens.txt", 93_939, "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d"),
        ),
        layout = TransducerLayout("encoder.int8.onnx", "decoder.int8.onnx", "joiner.int8.onnx", "tokens.txt"),
        recommendedRamBytes = 6L shl 30,
    )

    val ALL: List<AsrModel> = listOf(MOONSHINE_TINY, WHISPER_BASE_EN, WHISPER_SMALL_EN, PARAKEET_ULTRA, PARAKEET_V3)

    fun byId(id: String?): AsrModel? = ALL.firstOrNull { it.id == id }
}
