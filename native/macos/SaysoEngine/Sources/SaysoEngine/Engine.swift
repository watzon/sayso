// Engine state: downloads, loaded models, streaming sessions, and transcription.
import AVFoundation
import CoreML
import FluidAudio
import Foundation
import WhisperKit

/// A model that is held in memory.
enum LoadedModel {
    case parakeetUnified(batch: UnifiedAsrManager, stream: StreamingUnifiedAsrManager)
    case parakeetEou(StreamingEouAsrManager)
    case parakeetTdt(AsrManager)
    case nemotron(StreamingNemotronAsrManager)
    case nemotronMultilingual(StreamingNemotronMultilingualAsrManager)
    case cohere(CoherePipeline, CoherePipeline.LoadedModels)
    case canary(CanaryManager)
    case senseVoice(SenseVoiceModels)
    case paraformer(ParaformerManager)
    case whisper(WhisperKit)
    /// Apple Speech holds nothing in memory between dictations.
    case appleSpeech

    /// The streaming side of the model, if it has one.
    var streamer: Streamer? {
        switch self {
        case .parakeetUnified(_, let stream): return .standard(stream)
        case .parakeetEou(let eou): return .standard(eou)
        case .nemotron(let nemotron): return .standard(nemotron)
        case .nemotronMultilingual(let nemotron): return .nemotronMultilingual(nemotron)
        case .parakeetTdt, .cohere, .canary, .senseVoice, .paraformer, .whisper, .appleSpeech: return nil
        }
    }
}

/// One interface over the streaming managers. Nemotron multilingual has its own API
/// (a language prompt, float samples) and does not conform to `StreamingAsrManager`.
enum Streamer {
    case standard(any StreamingAsrManager)
    case nemotronMultilingual(StreamingNemotronMultilingualAsrManager)

    /// Clear the state of the last stream and set the language and the partial callback.
    func begin(language: String, onPartial: @escaping @Sendable (String) -> Void) async throws {
        switch self {
        case .standard(let manager):
            try await manager.reset()
            await manager.setPartialTranscriptCallback(onPartial)
        case .nemotronMultilingual(let manager):
            await manager.reset()
            let key = Self.promptKey(for: language, in: await manager.config.promptDictionary)
            await manager.setLanguage(key)
            await manager.setPartialCallback(onPartial)
        }
    }

    /// Add 16 kHz mono samples and decode what is complete.
    func feed(_ samples: [Float]) async throws {
        switch self {
        case .standard(let manager):
            guard let buffer = Self.buffer(from: samples) else { return }
            try await manager.appendAudio(buffer)
            try await manager.processBufferedAudio()
        case .nemotronMultilingual(let manager):
            _ = try await manager.process(samples: samples)
        }
    }

    func finish() async throws -> String {
        switch self {
        case .standard(let manager): return try await manager.finish()
        case .nemotronMultilingual(let manager): return try await manager.finish()
        }
    }

    /// The prompt key for a language code. The model lists some languages with a region
    /// only ("ja-JP"), so a plain code falls back to the first key with that prefix.
    static func promptKey(for language: String, in prompts: [String: Int]) -> String {
        if language.isEmpty || language == "auto" { return "auto" }
        if prompts[language] != nil { return language }
        return prompts.keys.filter { $0.hasPrefix(language + "-") }.sorted().first ?? "auto"
    }

    private static func buffer(from samples: [Float]) -> AVAudioPCMBuffer? {
        guard !samples.isEmpty,
            let format = AVAudioFormat(
                commonFormat: .pcmFormatFloat32, sampleRate: 16_000, channels: 1, interleaved: false),
            let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(samples.count))
        else { return nil }
        buffer.frameLength = AVAudioFrameCount(samples.count)
        samples.withUnsafeBufferPointer { source in
            buffer.floatChannelData![0].update(from: source.baseAddress!, count: samples.count)
        }
        return buffer
    }
}

/// One live preview stream.
private struct StreamSession {
    let streamer: Streamer
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
    ///
    /// FluidAudio gives the download phase the first half of its fraction and keeps the
    /// second half for a compile step that a download alone never runs.
    func stage(base: Double, weight: Double) -> ProgressHandler {
        { [self] progress in
            let done: Double
            switch progress.phase {
            case .listing: done = 0
            case .downloading: done = min(1, progress.fractionCompleted * 2)
            case .compiling: done = 1
            }
            report(base + weight * done)
        }
    }
}

actor Engine {
    private let store: ModelStore
    private let out: Output
    private var registry: [String: EngineSpec] = [:]
    private var loaded: [String: LoadedModel] = [:]
    private var busy: Set<String> = []
    private var sessions: [Int: StreamSession] = [:]
    /// Models whose streaming manager runs a final pass now. A new stream must wait.
    private var finalPasses: Set<String> = []

    private var ctcAssets: VocabularyBooster.CtcAssets?
    private var booster: VocabularyBooster?

    init(modelsDir: URL, out: Output) {
        self.store = ModelStore(root: modelsDir)
        self.out = out
    }

    // MARK: - Status

    private func state(of model: String, spec: EngineSpec) async -> String {
        if loaded[model] != nil { return "ready" }
        return await onDisk(spec) ? "downloaded" : "not_downloaded"
    }

    /// "Downloaded", for the files Sayso stores and for the Apple Speech assets of macOS.
    private func onDisk(_ spec: EngineSpec) async -> Bool {
        if spec == .appleSpeech {
            if #available(macOS 26, *) { return await AppleSpeech.anyInstalled() }
            return false
        }
        return store.isDownloaded(spec)
    }

    /// `list_models`: one `model_state` per model, from disk only.
    func listModels(_ models: [(id: String, spec: EngineSpec)], id: String?) async {
        for entry in models {
            registry[entry.id] = entry.spec
            out.send(
                "model_state", id: id,
                ["model": entry.id, "state": await state(of: entry.id, spec: entry.spec)])
        }
        out.ok(id: id)
    }

    // MARK: - Download

    private func acquire(_ model: String) throws {
        guard busy.insert(model).inserted else { throw EngineError("model \(model) is busy") }
    }

    func download(model: String, spec: EngineSpec, sizeBytes: Int64?, id: String?) async throws {
        registry[model] = spec
        if await onDisk(spec) {
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
            try store.markDownloaded(spec)
            guard await onDisk(spec) else {
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
        case .parakeetTdt(let version):
            // The v3 repo holds several encoders. The variant selects the int8 one.
            try await ModelHub.download(
                version.repo, to: store.root, variant: version == .v3 ? "int8" : nil,
                progressHandler: reporter.stage(base: 0, weight: 1))
            guard let folder = store.ownFolder(spec),
                AsrModels.modelsExist(at: folder, version: version.fluid)
            else { throw EngineError("download finished but the files are incomplete") }
        case .nemotron(let chunkMs):
            try await ModelHub.download(
                ModelStore.nemotronRepo(chunkMs: chunkMs), to: store.root,
                progressHandler: reporter.stage(base: 0, weight: 1))
        case .nemotronMultilingual(let chunkMs):
            try await ModelHub.download(
                .nemotronMultilingual,
                subdirectory: ModelStore.nemotronMultilingualVariant(chunkMs: chunkMs),
                to: store.root.appendingPathComponent(Repo.nemotronMultilingual.folderName, isDirectory: true),
                progressHandler: reporter.stage(base: 0, weight: 1),
                shouldSkip: { $0.contains(".mlpackage") })
        case .cohere:
            try await ModelHub.download(
                .cohereTranscribeCoreml, to: store.root,
                progressHandler: reporter.stage(base: 0, weight: 1))
        case .canary:
            try await ModelHub.download(
                .canary1bV2, to: store.root, variant: CanaryPrecision.int4.rawValue,
                progressHandler: reporter.stage(base: 0, weight: 1))
        case .senseVoice:
            try await ModelHub.download(
                .senseVoiceSmall, to: store.root, variant: SenseVoiceEncoderPrecision.int8.rawValue,
                progressHandler: reporter.stage(base: 0, weight: 1))
        case .paraformer:
            try await ModelHub.download(
                .paraformerLargeZh, to: store.root,
                progressHandler: reporter.stage(base: 0, weight: 1))
        case .appleSpeech:
            guard #available(macOS 26, *) else { throw EngineError("Apple Speech needs macOS 26") }
            // The assets of the language of this Mac. Other languages download on first use.
            try await AppleSpeech.install(language: "auto") { reporter.report($0) }
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

    /// `unload`: take a model out of memory. Its files stay on disk.
    func unload(model: String, id: String?) async throws {
        try acquire(model)
        defer { busy.remove(model) }
        if try await unload(model) {
            out.send("model_state", id: id, ["model": model, "state": "downloaded"])
        }
        out.ok(id: id)
    }

    /// Returns false when the model was not in memory.
    @discardableResult
    private func unload(_ model: String) async throws -> Bool {
        for (session, info) in sessions where info.model == model {
            sessions[session] = nil
        }
        guard let entry = loaded.removeValue(forKey: model) else { return false }
        switch entry {
        case .parakeetUnified(let batch, let stream):
            await batch.cleanup()
            await stream.cleanup()
        case .parakeetEou(let eou):
            await eou.cleanup()
        case .parakeetTdt(let manager):
            await manager.cleanup()
        case .nemotron(let manager):
            await manager.cleanup()
        case .nemotronMultilingual(let manager):
            await manager.cleanup()
        case .cohere, .canary, .senseVoice, .paraformer, .whisper, .appleSpeech:
            break  // Released with the last reference.
        }
        if case .parakeetUnified = entry {
            booster = nil
            ctcAssets = nil
        }
        return true
    }

    // MARK: - Load

    func load(model: String, spec: EngineSpec, id: String?) async throws {
        registry[model] = spec
        if loaded[model] != nil {
            out.send("model_state", id: id, ["model": model, "state": "ready", "load_ms": 0])
            out.ok(id: id)
            return
        }
        guard spec == .appleSpeech ? await onDisk(spec) : store.isLoadable(spec) else {
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
        case .parakeetTdt(let version):
            // The fused preprocessor and encoder of the 110M model returns only blanks on
            // the Neural Engine (seen on an M5 Pro with macOS 27: empty text, no error).
            // On the CPU it is correct and still runs at about 100 times real time.
            var configuration: MLModelConfiguration?
            if version == .tdtCtc110m {
                configuration = MLModelConfiguration()
                configuration?.computeUnits = .cpuOnly
            }
            let models = try await AsrModels.load(
                from: try folder(spec), configuration: configuration, version: version.fluid)
            let manager = AsrManager()
            try await manager.loadModels(models)
            return .parakeetTdt(manager)
        case .nemotron:
            let manager = StreamingNemotronAsrManager()
            try await manager.loadModels(from: try folder(spec))
            return .nemotron(manager)
        case .nemotronMultilingual:
            let manager = StreamingNemotronMultilingualAsrManager()
            try await manager.loadModels(from: try folder(spec))
            return .nemotronMultilingual(manager)
        case .cohere:
            let dir = try folder(spec)
            let models = try await CoherePipeline.loadModels(encoderDir: dir, decoderDir: dir, vocabDir: dir)
            return .cohere(CoherePipeline(), models)
        case .canary:
            return .canary(CanaryManager(models: try CanaryModels.load(from: try folder(spec), precision: .int4)))
        case .senseVoice:
            return .senseVoice(try SenseVoiceModels.load(from: try folder(spec), precision: .int8))
        case .paraformer:
            return .paraformer(ParaformerManager(models: try ParaformerModels.load(from: try folder(spec))))
        case .appleSpeech:
            return .appleSpeech
        }
    }

    private func folder(_ spec: EngineSpec) throws -> URL {
        guard let folder = store.ownFolder(spec) else { throw EngineError("the model has no folder") }
        return folder
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
        case .parakeetTdt(let manager):
            var state = TdtDecoderState.make(decoderLayers: await manager.decoderLayerCount)
            // The language is a hint for the v3 family. The other versions ignore it.
            let result = try await manager.transcribe(
                samples, decoderState: &state, language: Language(rawValue: language))
            text = result.text
        case .nemotron(let manager):
            text = try await transcribeStreaming(
                model: model, streamer: .standard(manager), samples: samples, language: language)
        case .nemotronMultilingual(let manager):
            // The words bias the decoder. An empty list turns the bias off.
            await manager.setCustomVocabulary(vocabulary.map { CustomVocabularyTerm(text: $0) })
            text = try await transcribeStreaming(
                model: model, streamer: .nemotronMultilingual(manager), samples: samples, language: language)
        case .cohere(let pipeline, let models):
            // Cohere needs a language. Detection is not possible, so "auto" means English.
            let result = try await pipeline.transcribeLong(
                audio: samples, models: models,
                language: CohereAsrConfig.Language(rawValue: language) ?? .english)
            text = result.text
        case .canary(let manager):
            text = try await manager.transcribe(audio: samples)
        case .senseVoice(let models):
            // Text norm 14 is "with ITN": the model writes punctuation, capitals, and digits.
            let manager = SenseVoiceManager(
                models: models, language: Self.senseVoiceLanguage(language), textNorm: 14)
            var parts: [String] = []
            for chunk in AudioChunks.split(samples, maxSeconds: 25) {
                parts.append(try await manager.transcribe(audio: chunk))
            }
            text = AudioChunks.join(parts)
        case .paraformer(let manager):
            var parts: [String] = []
            for chunk in AudioChunks.split(samples, maxSeconds: 25) {
                parts.append(try await manager.transcribe(audio: chunk))
            }
            text = AudioChunks.join(parts)
        case .appleSpeech:
            guard #available(macOS 26, *) else { throw EngineError("Apple Speech needs macOS 26") }
            text = try await AppleSpeech.transcribe(
                url: URL(fileURLWithPath: path), language: language, vocabulary: vocabulary)
        case .parakeetEou:
            throw EngineError("model \(model) has no final pass")
        }
        out.send(
            "final", id: id,
            ["text": text.trimmingCharacters(in: .whitespacesAndNewlines), "elapsed_ms": Int(nowMs() - start)])
    }

    /// The language id of SenseVoice. Zero lets the model detect the language.
    private static func senseVoiceLanguage(_ code: String) -> Int32 {
        switch code {
        case "zh": return 3
        case "en": return 4
        case "yue": return 7
        case "ja": return 11
        case "ko": return 12
        default: return SenseVoiceConfig.defaultLanguage
        }
    }

    /// Final pass with a streaming model: feed the whole recording through its stream.
    private func transcribeStreaming(
        model: String, streamer: Streamer, samples: [Float], language: String
    ) async throws -> String {
        // The live preview of the same dictation uses the same manager. Its `stream_end`
        // can arrive after this request, so wait for it.
        let deadline = nowMs() + 5000
        while sessions.values.contains(where: { $0.model == model }) || finalPasses.contains(model) {
            guard nowMs() < deadline else { throw EngineError("model \(model) is busy with another stream") }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
        finalPasses.insert(model)
        defer { finalPasses.remove(model) }
        try await streamer.begin(language: language, onPartial: { _ in })
        try await streamer.feed(samples)
        return try await streamer.finish()
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

    func streamStart(session: Int, model: String, language: String) async throws {
        guard let entry = loaded[model] else { throw EngineError("model \(model) is not loaded") }
        guard let streamer = entry.streamer else {
            throw EngineError("model \(model) has no live preview")
        }
        guard !finalPasses.contains(model) else { throw EngineError("model \(model) is busy with a final pass") }
        // One live stream per manager. A new session replaces an older one.
        for (other, info) in sessions where info.model == model { sessions[other] = nil }
        let out = self.out
        let tracker = PartialTracker()
        try await streamer.begin(language: language) { text in
            // The streaming decoders only append. There is no tentative tail to report.
            tracker.set(text)
            out.send("partial", ["session": session, "committed": text, "tentative": ""])
        }
        sessions[session] = StreamSession(streamer: streamer, model: model, tracker: tracker)
    }

    func streamAudio(session: Int, pcm: String) async throws {
        guard let info = sessions[session] else {
            throw EngineError("unknown stream session \(session)")
        }
        guard let data = Data(base64Encoded: pcm) else { throw EngineError("pcm is not base64") }
        try await info.streamer.feed(Self.samples(fromInt16LE: data))
    }

    /// Finish a stream. Returns the final preview text.
    func streamEnd(session: Int) async throws -> String {
        guard let info = sessions[session] else {
            throw EngineError("unknown stream session \(session)")
        }
        // The session stays registered until the manager is done, so a final pass on the
        // same manager waits for it.
        defer { sessions[session] = nil }
        let text = try await info.streamer.finish()
        if text != info.tracker.last {
            out.send("partial", ["session": session, "committed": text, "tentative": ""])
        }
        return text
    }

    /// 16 kHz mono little-endian Int16 bytes to float samples.
    private static func samples(fromInt16LE data: Data) -> [Float] {
        let count = data.count / 2
        var samples = [Float](repeating: 0, count: count)
        data.withUnsafeBytes { raw in
            for i in 0..<count {
                let sample = Int16(littleEndian: raw.loadUnaligned(fromByteOffset: i * 2, as: Int16.self))
                samples[i] = Float(sample) / 32768.0
            }
        }
        return samples
    }
}

/// Cuts a long recording into pieces for the models that take one fixed window.
enum AudioChunks {
    static let sampleRate = 16_000

    /// Pieces of at most `maxSeconds`. Each cut is at the quietest point in the last
    /// five seconds of a piece, so it rarely falls inside a word.
    static func split(_ samples: [Float], maxSeconds: Int) -> [[Float]] {
        let limit = maxSeconds * sampleRate
        guard samples.count > limit else { return [samples] }
        let window = sampleRate / 5  // 200 ms
        let search = 5 * sampleRate
        var pieces: [[Float]] = []
        var start = 0
        while samples.count - start > limit {
            let end = start + limit
            var best = end
            var bestEnergy = Float.greatestFiniteMagnitude
            var position = end - search
            while position + window <= end {
                var energy: Float = 0
                for i in position..<(position + window) { energy += samples[i] * samples[i] }
                if energy < bestEnergy {
                    bestEnergy = energy
                    best = position + window / 2
                }
                position += window / 2
            }
            pieces.append(Array(samples[start..<best]))
            start = best
        }
        pieces.append(Array(samples[start...]))
        return pieces
    }

    /// Join the texts of the pieces. A space goes between two pieces only when the left
    /// one ends in a script that uses spaces (not Chinese or Japanese).
    static func join(_ parts: [String]) -> String {
        var result = ""
        for part in parts.map({ $0.trimmingCharacters(in: .whitespacesAndNewlines) }) where !part.isEmpty {
            if let last = result.unicodeScalars.last, last.value < 0x2E80 { result += " " }
            result += part
        }
        return result
    }
}
