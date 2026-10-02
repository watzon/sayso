// Engine state: downloads, loaded models, streaming sessions, and transcription.
import AVFoundation
import FluidAudio
import Foundation
import WhisperKit

/// A model that is held in memory.
enum LoadedModel {
    case parakeetUnified(batch: UnifiedAsrManager, stream: StreamingUnifiedAsrManager)
    case parakeetEou(StreamingEouAsrManager)
    case whisper(WhisperKit)

    /// The manager that serves live preview, if the model has one.
    var streamingManager: (any StreamingAsrManager)? {
        switch self {
        case .parakeetUnified(_, let stream): return stream
        case .parakeetEou(let eou): return eou
        case .whisper: return nil
        }
    }
}

/// One live preview stream.
private struct StreamSession {
    let manager: any StreamingAsrManager
    let model: String
    let tracker: PartialTracker
}

/// Remembers the last partial text of a stream, so the end of the stream does not repeat it.
final class PartialTracker: @unchecked Sendable {
    private let lock = NSLock()
    private var text = ""

    var last: String {
        lock.lock()
        defer { lock.unlock() }
        return text
    }

    func set(_ value: String) {
        lock.lock()
        text = value
        lock.unlock()
    }
}

/// Turns library progress into throttled `model_state` and `download_progress` messages.
/// At most 8 messages per second, and the fraction never goes backwards.
final class DownloadReporter: @unchecked Sendable {
    private let out: Output
    private let id: String?
    private let model: String
    private let totalBytes: Int64
    private let lock = NSLock()
    private var lastEmit = 0.0
    private var highest = 0.0
    private static let minIntervalMs = 125.0

    init(out: Output, id: String?, model: String, totalBytes: Int64) {
        self.out = out
        self.id = id
        self.model = model
        self.totalBytes = totalBytes
    }

    /// Report overall progress in 0...1. `force` skips the throttle (first and last message).
    func report(_ fraction: Double, force: Bool = false) {
        lock.lock()
        highest = max(highest, min(1, max(0, fraction)))
        let value = highest
        let now = nowMs()
        guard force || now - lastEmit >= Self.minIntervalMs else {
            lock.unlock()
            return
        }
        lastEmit = now
        lock.unlock()

        let done = Int64(Double(totalBytes) * value)
        let fields: [String: Any] = [
            "model": model, "fraction": value, "bytes_done": done, "bytes_total": totalBytes,
        ]
        out.send("download_progress", id: id, fields)
        var state = fields
        state["state"] = "downloading"
        out.send("model_state", id: id, state)
    }

    /// A FluidAudio progress handler for one stage that covers `weight` of the whole download.
    func stage(base: Double, weight: Double) -> ProgressHandler {
        { [self] progress in report(base + weight * progress.fractionCompleted) }
    }
}

actor Engine {
    private let store: ModelStore
    private let out: Output
    private var registry: [String: EngineSpec] = [:]
    private var loaded: [String: LoadedModel] = [:]
    private var busy: Set<String> = []
    private var sessions: [Int: StreamSession] = [:]

    private var ctcAssets: VocabularyBooster.CtcAssets?
    private var booster: VocabularyBooster?

    init(modelsDir: URL, out: Output) {
        self.store = ModelStore(root: modelsDir)
        self.out = out
    }

    // MARK: - Status

    private func state(of model: String, spec: EngineSpec) -> String {
        if loaded[model] != nil { return "ready" }
        return store.isDownloaded(spec) ? "downloaded" : "not_downloaded"
    }

    /// `list_models`: one `model_state` per model, from disk only.
    func listModels(_ models: [(id: String, spec: EngineSpec)], id: String?) {
        for entry in models {
            registry[entry.id] = entry.spec
            out.send(
                "model_state", id: id,
                ["model": entry.id, "state": state(of: entry.id, spec: entry.spec)])
        }
        out.ok(id: id)
    }

    // MARK: - Download

    private func acquire(_ model: String) throws {
        guard busy.insert(model).inserted else { throw EngineError("model \(model) is busy") }
    }

    func download(model: String, spec: EngineSpec, sizeBytes: Int64?, id: String?) async throws {
        registry[model] = spec
        if store.isDownloaded(spec) {
            out.send("model_state", id: id, ["model": model, "state": "downloaded"])
            out.ok(id: id)
            return
        }
        try acquire(model)
        defer { busy.remove(model) }

        let reporter = DownloadReporter(
            out: out, id: id, model: model, totalBytes: sizeBytes ?? 0)
        reporter.report(0, force: true)
        do {
            try await fetch(spec, reporter: reporter)
            guard store.isDownloaded(spec) else {
                throw EngineError("download finished but the files are incomplete")
            }
        } catch {
            out.send("model_state", id: id, ["model": model, "state": "failed", "message": "\(error)"])
            throw error
        }
        reporter.report(1, force: true)
        out.send("model_state", id: id, ["model": model, "state": "downloaded"])
        out.ok(id: id)
    }

    /// Download what is missing. Each stage is skipped without a network call when its files exist.
    private func fetch(_ spec: EngineSpec, reporter: DownloadReporter) async throws {
        switch spec {
        case .parakeetUnified(let tier):
            guard let config = ModelStore.unifiedConfig(tier: tier) else {
                throw EngineError("bad streaming tier \(tier)")
            }
            // Weights follow the catalog sizes: 595 MB batch, 578 MB streaming, 98 MB CTC helper.
            let stages: [(weight: Double, done: Bool)] = [
                (0.47, store.batchEncoderComplete()),
                (0.45, store.streamingEncoderComplete(config)),
                (0.08, store.ctcComplete()),
            ]
            var base = 0.0
            if !stages[0].done {
                try await ModelHub.download(
                    .parakeetUnified, to: store.root, variant: "offline",
                    additionalModelNames: [ModelStore.offlineEncoder],
                    progressHandler: reporter.stage(base: base, weight: stages[0].weight))
            }
            base += stages[0].weight
            reporter.report(base)
            if !stages[1].done {
                try await ModelHub.download(
                    .parakeetUnified, to: store.root, variant: nil,
                    additionalModelNames: [ModelStore.streamingEncoder(config)],
                    progressHandler: reporter.stage(base: base, weight: stages[1].weight))
            }
            base += stages[1].weight
            reporter.report(base)
            if !stages[2].done {
                // CTC 110M: the vocabulary helper for FluidAudio vocabulary boosting.
                try await CtcModels.download(to: store.ctcDir, variant: .ctc110m)
            }
        case .parakeetEou:
            try await ModelHub.download(
                .parakeetEou160, to: store.root,
                progressHandler: reporter.stage(base: 0, weight: 1))
        case .whisper(let variant):
            _ = try await WhisperKit.download(variant: variant, downloadBase: store.root) { progress in
                reporter.report(progress.fractionCompleted * 0.98)
            }
            // The tokenizer lives in a separate Hub repo. Fetch it now so load needs no network.
            if let tokenizer = ModelStore.whisperTokenizer(variant: variant) {
                _ = try await ModelUtilities.loadTokenizer(
                    for: tokenizer.model, tokenizerFolder: store.root)
            }
        }
    }

    // MARK: - Delete

    func delete(model: String, spec: EngineSpec?, id: String?) async throws {
        guard let spec = spec ?? registry[model] else {
            throw EngineError("unknown model \(model): send list_models or an engine field first")
        }
        try acquire(model)
        defer { busy.remove(model) }
        try await unload(model)
        try store.delete(spec)
        out.send("model_state", id: id, ["model": model, "state": "not_downloaded"])
        out.ok(id: id)
    }

    private func unload(_ model: String) async throws {
        for (session, info) in sessions where info.model == model {
            sessions[session] = nil
        }
        guard let entry = loaded.removeValue(forKey: model) else { return }
        switch entry {
        case .parakeetUnified(let batch, let stream):
            await batch.cleanup()
            await stream.cleanup()
        case .parakeetEou(let eou):
            await eou.cleanup()
        case .whisper:
            break  // Released with the last reference.
        }
        if case .parakeetUnified = entry {
            booster = nil
            ctcAssets = nil
        }
    }

    // MARK: - Load

    func load(model: String, spec: EngineSpec, id: String?) async throws {
        registry[model] = spec
        if loaded[model] != nil {
            out.send("model_state", id: id, ["model": model, "state": "ready", "load_ms": 0])
            out.ok(id: id)
            return
        }
        guard store.isLoadable(spec) else {
            throw EngineError("model \(model) is not downloaded")
        }
        try acquire(model)
        defer { busy.remove(model) }

        // First load compiles the model for the Neural Engine and can take a minute.
        out.send("model_state", id: id, ["model": model, "state": "optimizing"])
        let start = nowMs()
        do {
            loaded[model] = try await loadModel(spec)
        } catch {
            out.send("model_state", id: id, ["model": model, "state": "failed", "message": "\(error)"])
            throw error
        }
        out.send(
            "model_state", id: id,
            ["model": model, "state": "ready", "load_ms": Int(nowMs() - start)])
        out.ok(id: id)
    }

    private func loadModel(_ spec: EngineSpec) async throws -> LoadedModel {
        switch spec {
        case .parakeetUnified(let tier):
            guard let config = ModelStore.unifiedConfig(tier: tier) else {
                throw EngineError("bad streaming tier \(tier)")
            }
            let batch = UnifiedAsrManager()
            try await batch.loadModels(from: store.unifiedDir)
            let stream = StreamingUnifiedAsrManager(config: config)
            try await stream.loadModels(from: store.unifiedDir)
            return .parakeetUnified(batch: batch, stream: stream)
        case .parakeetEou:
            let eou = StreamingEouAsrManager(chunkSize: .ms160)
            try await eou.loadModels(from: store.eouDir)
            return .parakeetEou(eou)
        case .whisper(let variant):
            let config = WhisperKitConfig(
                downloadBase: store.root, modelFolder: store.whisperDir(variant).path,
                tokenizerFolder: store.root, verbose: false, logLevel: .none,
                load: true, download: false)
            return .whisper(try await WhisperKit(config))
        }
    }

    // MARK: - Transcribe

    func transcribe(
        model: String, path: String, language: String, vocabulary: [String], id: String?
    ) async throws {
        guard let entry = loaded[model] else { throw EngineError("model \(model) is not loaded") }
        let samples = try AudioConverter().resampleAudioFile(path: path)
        guard !samples.isEmpty else { throw EngineError("audio file has no samples") }

        let start = nowMs()
        let text: String
        switch entry {
        case .parakeetUnified(let batch, _):
            text = try await transcribeParakeet(batch, samples: samples, vocabulary: vocabulary)
        case .whisper(let kit):
            text = try await transcribeWhisper(
                kit, samples: samples, language: language, vocabulary: vocabulary)
        case .parakeetEou:
            throw EngineError("model \(model) has no final pass")
        }
        out.send("final", id: id, ["text": text, "elapsed_ms": Int(nowMs() - start)])
    }

    private func transcribeParakeet(
        _ batch: UnifiedAsrManager, samples: [Float], vocabulary: [String]
    ) async throws -> String {
        if !vocabulary.isEmpty, let booster = await boosterFor(vocabulary) {
            let result = try await batch.transcribeWithTimings(samples)
            return await booster.rescore(
                text: result.text, tokenTimings: result.tokenTimings, samples: samples)
        }
        return try await batch.transcribe(samples)
    }

    /// The booster for a word list, cached while the list stays the same.
    /// Nil (with a log line) when the CTC helper is missing.
    private func boosterFor(_ words: [String]) async -> VocabularyBooster? {
        if let booster, booster.terms == words { return booster }
        do {
            if ctcAssets == nil {
                guard store.ctcComplete() else {
                    out.log("warn", "vocabulary ignored: CTC helper model is not downloaded")
                    return nil
                }
                ctcAssets = try await VocabularyBooster.CtcAssets.load(from: store.ctcDir)
            }
            let built = try await VocabularyBooster(words: words, assets: ctcAssets!)
            booster = built
            return built
        } catch {
            out.log("warn", "vocabulary ignored: \(error)")
            return nil
        }
    }

    private func transcribeWhisper(
        _ kit: WhisperKit, samples: [Float], language: String, vocabulary: [String]
    ) async throws -> String {
        let detect = language.isEmpty || language == "auto"
        var options = DecodingOptions(
            language: detect ? nil : language, detectLanguage: detect)
        // Dictionary words go in as prompt text. Whisper has no hard boosting.
        if !vocabulary.isEmpty, let tokenizer = kit.tokenizer {
            let prompt = " " + vocabulary.joined(separator: ", ")
            options.promptTokens = tokenizer.encode(text: prompt)
                .filter { $0 < tokenizer.specialTokens.specialTokenBegin }
            options.usePrefillPrompt = true
        }
        let results = try await kit.transcribe(audioArray: samples, decodeOptions: options)
        return results.map(\.text).joined(separator: " ").trimmingCharacters(in: .whitespacesAndNewlines)
    }

    // MARK: - Streaming

    func streamStart(session: Int, model: String) async throws {
        guard let entry = loaded[model] else { throw EngineError("model \(model) is not loaded") }
        guard let manager = entry.streamingManager else {
            throw EngineError("model \(model) has no live preview")
        }
        // One live stream per manager. A new session replaces an older one.
        for (other, info) in sessions where info.model == model { sessions[other] = nil }
        try await manager.reset()
        let out = self.out
        let tracker = PartialTracker()
        await manager.setPartialTranscriptCallback { text in
            // The Parakeet streaming decoders only append. There is no tentative tail to report.
            tracker.set(text)
            out.send("partial", ["session": session, "committed": text, "tentative": ""])
        }
        sessions[session] = StreamSession(manager: manager, model: model, tracker: tracker)
    }

    func streamAudio(session: Int, pcm: String) async throws {
        guard let info = sessions[session] else {
            throw EngineError("unknown stream session \(session)")
        }
        guard let data = Data(base64Encoded: pcm) else { throw EngineError("pcm is not base64") }
        guard let buffer = Self.buffer(fromInt16LE: data) else { return }
        try await info.manager.appendAudio(buffer)
        try await info.manager.processBufferedAudio()
    }

    /// Finish a stream. Returns the final preview text.
    func streamEnd(session: Int) async throws -> String {
        guard let info = sessions.removeValue(forKey: session) else {
            throw EngineError("unknown stream session \(session)")
        }
        let text = try await info.manager.finish()
        if text != info.tracker.last {
            out.send("partial", ["session": session, "committed": text, "tentative": ""])
        }
        return text
    }

    /// 16 kHz mono little-endian Int16 bytes to a Float32 buffer.
    private static func buffer(fromInt16LE data: Data) -> AVAudioPCMBuffer? {
        let count = data.count / 2
        guard count > 0,
            let format = AVAudioFormat(
                commonFormat: .pcmFormatFloat32, sampleRate: 16_000, channels: 1, interleaved: false),
            let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(count))
        else { return nil }
        buffer.frameLength = AVAudioFrameCount(count)
        let target = buffer.floatChannelData![0]
        data.withUnsafeBytes { raw in
            for i in 0..<count {
                let sample = Int16(littleEndian: raw.loadUnaligned(fromByteOffset: i * 2, as: Int16.self))
                target[i] = Float(sample) / 32768.0
            }
        }
        return buffer
    }
}
