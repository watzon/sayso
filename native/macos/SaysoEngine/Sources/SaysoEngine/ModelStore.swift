// Where models live under SAYSO_MODELS_DIR, and the local (no network) completeness checks.
import FluidAudio
import Foundation
import WhisperKit

/// Disk layout under the models directory. Folder names come from the libraries so the
/// layout stays in sync with what their downloaders write.
struct ModelStore {
    let root: URL

    private let fm = FileManager.default

    // MARK: Folders

    /// FluidAudio folder for Parakeet Unified. Batch and streaming encoders share it.
    var unifiedDir: URL { root.appendingPathComponent(Repo.parakeetUnified.folderName, isDirectory: true) }
    /// FluidAudio folder for the CTC 110M vocabulary helper.
    var ctcDir: URL { root.appendingPathComponent(Repo.parakeetCtc110m.folderName, isDirectory: true) }
    /// FluidAudio folder for Parakeet EOU (160 ms tier).
    var eouDir: URL { root.appendingPathComponent(Repo.parakeetEou160.folderName, isDirectory: true) }
    /// WhisperKit model folder for one variant.
    func whisperDir(_ variant: String) -> URL {
        root.appendingPathComponent("models/argmaxinc/whisperkit-coreml/\(variant)", isDirectory: true)
    }

    private func repoDir(_ repo: Repo) -> URL {
        root.appendingPathComponent(repo.folderName, isDirectory: true)
    }

    /// The folder that holds every file of a model, for the families where one model
    /// owns one folder. Nil for Parakeet Unified, Parakeet EOU, and Whisper, which
    /// have their own layout above.
    func ownFolder(_ spec: EngineSpec) -> URL? {
        switch spec {
        case .parakeetUnified, .parakeetEou, .whisper, .appleSpeech: return nil
        case .parakeetTdt(let version): return repoDir(version.repo)
        case .nemotron(let chunkMs): return repoDir(Self.nemotronRepo(chunkMs: chunkMs))
        case .nemotronMultilingual(let chunkMs):
            return repoDir(.nemotronMultilingual)
                .appendingPathComponent(Self.nemotronMultilingualVariant(chunkMs: chunkMs), isDirectory: true)
        case .cohere: return repoDir(.cohereTranscribeCoreml)
        case .canary: return repoDir(.canary1bV2)
        case .senseVoice: return repoDir(.senseVoiceSmall)
        case .paraformer: return repoDir(.paraformerLargeZh)
        }
    }

    static func nemotronRepo(chunkMs: Int) -> Repo {
        switch chunkMs {
        case 560: return .nemotronStreaming560
        case 2240: return .nemotronStreaming2240
        default: return .nemotronStreaming1120
        }
    }

    /// The Hub subfolder of one Nemotron multilingual build. The "multilingual" build
    /// has the full vocabulary. The smaller "latin" build has six languages only.
    static func nemotronMultilingualVariant(chunkMs: Int) -> String { "multilingual/\(chunkMs)ms" }

    // MARK: Download marker

    /// FluidAudio stages each file as `*.partial` and creates bundle folders early, so
    /// a folder that exists can still be half a download. The engine writes this file
    /// when a download ends without an error. Only then is the model "downloaded".
    private static let marker = ".sayso-downloaded"

    func markDownloaded(_ spec: EngineSpec) throws {
        guard let folder = ownFolder(spec) else { return }
        try Data().write(to: folder.appendingPathComponent(Self.marker))
    }

    private func isMarked(_ spec: EngineSpec) -> Bool {
        guard let folder = ownFolder(spec) else { return false }
        return fileExists(folder.appendingPathComponent(Self.marker))
    }

    // MARK: Parakeet Unified file names

    static let offlineEncoder = ModelNames.ParakeetUnified.offlineEncoderFile(precision: .int8)
    static let sharedUnifiedFiles = [
        ModelNames.ParakeetUnified.decoderFile,
        ModelNames.ParakeetUnified.jointDecisionFile,
    ]

    /// Parse a tier id such as "70_2_2" into a config. Nil when the id is malformed.
    static func unifiedConfig(tier: String) -> UnifiedConfig? {
        let parts = tier.split(separator: "_").compactMap { Int($0) }
        guard parts.count == 3 else { return nil }
        return UnifiedConfig(leftFrames: parts[0], chunkFrames: parts[1], rightFrames: parts[2])
    }

    static func streamingEncoder(_ config: UnifiedConfig) -> String {
        ModelNames.ParakeetUnified.streamingEncoderFile(precision: .int8, contextSuffix: config.contextSuffix)
    }

    // MARK: Completeness

    /// A compiled Core ML bundle is complete when its manifest and graph exist.
    private func bundleComplete(_ url: URL) -> Bool {
        fm.fileExists(atPath: url.appendingPathComponent("coremldata.bin").path)
            && fm.fileExists(atPath: url.appendingPathComponent("model.mil").path)
    }

    private func fileExists(_ url: URL) -> Bool { fm.fileExists(atPath: url.path) }

    func batchEncoderComplete() -> Bool {
        bundleComplete(unifiedDir.appendingPathComponent(Self.offlineEncoder))
            && sharedUnifiedComplete()
    }

    func streamingEncoderComplete(_ config: UnifiedConfig) -> Bool {
        bundleComplete(unifiedDir.appendingPathComponent(Self.streamingEncoder(config)))
            && sharedUnifiedComplete()
    }

    private func sharedUnifiedComplete() -> Bool {
        Self.sharedUnifiedFiles.allSatisfy { bundleComplete(unifiedDir.appendingPathComponent($0)) }
            && fileExists(unifiedDir.appendingPathComponent(ModelNames.ParakeetUnified.vocab))
    }

    func ctcComplete() -> Bool {
        CtcModels.modelsExist(at: ctcDir) && fileExists(ctcDir.appendingPathComponent("tokenizer.json"))
    }

    func eouComplete() -> Bool {
        ["streaming_encoder.mlmodelc", "decoder.mlmodelc", "joint_decision.mlmodelc"]
            .allSatisfy { bundleComplete(eouDir.appendingPathComponent($0)) }
            && fileExists(eouDir.appendingPathComponent("vocab.json"))
    }

    func whisperComplete(_ variant: String) -> Bool {
        let dir = whisperDir(variant)
        let bundles = ["AudioEncoder.mlmodelc", "MelSpectrogram.mlmodelc", "TextDecoder.mlmodelc"]
        guard bundles.allSatisfy({ bundleComplete(dir.appendingPathComponent($0)) }),
            fileExists(dir.appendingPathComponent("config.json"))
        else { return false }
        if let tokenizer = Self.whisperTokenizer(variant: variant) {
            return fileExists(
                root.appendingPathComponent("models/\(tokenizer.repo)/tokenizer.json"))
        }
        return true
    }

    /// Everything the model needs, including helpers. This is what "downloaded" means.
    func isDownloaded(_ spec: EngineSpec) -> Bool {
        switch spec {
        case .parakeetUnified(let tier):
            guard let config = Self.unifiedConfig(tier: tier) else { return false }
            return batchEncoderComplete() && streamingEncoderComplete(config) && ctcComplete()
        case .parakeetEou:
            return eouComplete()
        case .whisper(let variant):
            return whisperComplete(variant)
        case .parakeetTdt, .nemotron, .nemotronMultilingual, .cohere, .canary, .senseVoice, .paraformer:
            return isMarked(spec)
        case .appleSpeech:
            return false  // macOS holds the assets. The engine asks macOS (see `Engine.onDisk`).
        }
    }

    /// Enough files to load and transcribe. The CTC helper is optional (vocabulary boosting only).
    func isLoadable(_ spec: EngineSpec) -> Bool {
        switch spec {
        case .parakeetUnified(let tier):
            guard let config = Self.unifiedConfig(tier: tier) else { return false }
            return batchEncoderComplete() && streamingEncoderComplete(config)
        case .parakeetEou:
            return eouComplete()
        case .whisper(let variant):
            return whisperComplete(variant)
        case .parakeetTdt, .nemotron, .nemotronMultilingual, .cohere, .canary, .senseVoice, .paraformer:
            return isMarked(spec)
        case .appleSpeech:
            return false
        }
    }

    // MARK: Delete

    /// Remove the files of one model. Helpers owned by the model go with it.
    func delete(_ spec: EngineSpec) throws {
        switch spec {
        case .parakeetUnified:
            try remove(unifiedDir)
            try remove(ctcDir)
        case .parakeetEou:
            try remove(eouDir)
        case .whisper(let variant):
            // The tokenizer folder is shared between variants, so it stays.
            try remove(whisperDir(variant))
        case .parakeetTdt, .nemotron, .nemotronMultilingual, .cohere, .canary, .senseVoice, .paraformer:
            if let folder = ownFolder(spec) { try remove(folder) }
        case .appleSpeech:
            throw EngineError("macOS manages the Apple Speech files. Sayso cannot delete them.")
        }
    }

    private func remove(_ url: URL) throws {
        if fm.fileExists(atPath: url.path) { try fm.removeItem(at: url) }
    }

    // MARK: Whisper tokenizer

    /// The tokenizer WhisperKit needs for a variant, as a Hub repo id and a `ModelVariant`.
    /// Nil when the variant is not recognised (WhisperKit then fetches it on its own at load).
    static func whisperTokenizer(variant: String) -> (repo: String, model: ModelVariant)? {
        let name = variant.lowercased()
        // Distil large-v3 uses the large-v3 tokenizer.
        if name.contains("large-v3") { return ("openai/whisper-large-v3", .largev3) }
        if name.contains("large-v2") { return ("openai/whisper-large-v2", .largev2) }
        if name.contains("medium.en") { return ("openai/whisper-medium.en", .mediumEn) }
        if name.contains("medium") { return ("openai/whisper-medium", .medium) }
        if name.contains("small.en") { return ("openai/whisper-small.en", .smallEn) }
        if name.contains("small") { return ("openai/whisper-small", .small) }
        if name.contains("base.en") { return ("openai/whisper-base.en", .baseEn) }
        if name.contains("base") { return ("openai/whisper-base", .base) }
        if name.contains("tiny.en") { return ("openai/whisper-tiny.en", .tinyEn) }
        if name.contains("tiny") { return ("openai/whisper-tiny", .tiny) }
        return nil
    }
}

extension TdtVersion {
    var fluid: AsrModelVersion {
        switch self {
        case .v2: return .v2
        case .v3: return .v3
        case .ultra: return .ultra
        case .redux: return .redux
        case .phonon2: return .phonon2
        case .tdtCtc110m: return .tdtCtc110m
        case .ja: return .tdtJa
        }
    }

    var repo: Repo {
        switch self {
        case .v2: return .parakeetV2
        case .v3: return .parakeetV3
        case .ultra: return .parakeetUltra
        case .redux: return .parakeetRedux
        case .phonon2: return .phonon2
        case .tdtCtc110m: return .parakeetTdtCtc110m
        case .ja: return .parakeetJa
        }
    }
}
