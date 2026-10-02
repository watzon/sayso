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
