// Engine state: downloads, loaded models, streaming sessions, and transcription.
import AVFoundation
import CoreML
import FluidAudio
import Foundation
import WhisperKit

/// Parakeet Unified in memory. The two parts weigh about 595 MB (batch) and 578 MB
/// (stream), and a model is often only the final model or only the preview model.
///
/// The batch part loads with the model, because the final pass waits for the model to be
/// ready and must not wait for a load after that. The stream part loads at the first live
/// preview (see `Engine.streamStart`), so a final model with no preview never holds it.
/// Each other stream at the same time loads a copy of the stream part (see `StreamerPool`).
final class UnifiedModel: @unchecked Sendable {
    let batch: UnifiedAsrManager
    let stream: StreamerPool<Streamer>
    let vocabulary: KeyedParts<[String], VocabularyBooster>
    private let ctc: LazyPart<VocabularyBooster.CtcAssets>

    init(
        batch: UnifiedAsrManager, config: UnifiedConfig, directory: URL, ctcDirectory: URL,
        ctcComplete: @escaping @Sendable () -> Bool, out: Output
    ) {
        let assets = LazyPart<VocabularyBooster.CtcAssets>(
            load: {
                guard ctcComplete() else { throw EngineError("CTC helper model is not downloaded") }
                let start = nowMs()
                let assets = try await VocabularyBooster.CtcAssets.load(from: ctcDirectory)
                out.log("debug", "dictionary assets loaded in \(Int(nowMs() - start)) ms")
                return assets
            }, free: { _ in })
        ctc = assets
        vocabulary = KeyedParts { words in
            try await VocabularyBooster(words: words, assets: assets.get())
        }
        self.batch = batch
        stream = StreamerPool(
            load: {
                let manager = StreamingUnifiedAsrManager(config: config)
                let start = nowMs()
                try await manager.loadModels(from: directory)
                out.log("debug", "preview part loaded in \(Int(nowMs() - start)) ms")
                return .standard(manager)
            })
    }

    /// Free both parts.
    func cleanup() async {
        await batch.cleanup()
        await stream.close()
        await vocabulary.close()
        await ctc.close()
    }
}

/// A model that is held in memory.
enum LoadedModel {
    case parakeetUnified(UnifiedModel)
    case parakeetEou(StreamerPool<Streamer>)
    case parakeetTdt(AsrManager)
    case nemotron(StreamerPool<Streamer>)
    case nemotronMultilingual(StreamerPool<Streamer>)
    case cohere(CoherePipeline, CoherePipeline.LoadedModels)
    case canary(CanaryManager)
    case senseVoice(SenseVoiceModels)
    case paraformer(ParaformerManager)
    case whisper(WhisperKit)
    /// Apple Speech holds nothing in memory between dictations.
    case appleSpeech
    /// The Silero VAD. Its sessions share it: each `VadStream` keeps its own state.
    case vad(VadManager)
    /// The models of the offline diarizer. Each `diarize` makes a manager with its own settings.
    case diarization(OfflineDiarizerModels)

    /// The streaming managers of the model, if it has a streaming side.
    var streamers: StreamerPool<Streamer>? {
        switch self {
        case .parakeetUnified(let unified): return unified.stream
        case .parakeetEou(let pool), .nemotron(let pool), .nemotronMultilingual(let pool): return pool
        case .parakeetTdt, .cohere, .canary, .senseVoice, .paraformer, .whisper, .appleSpeech, .vad, .diarization:
            return nil
        }
    }

    var task: ModelTask {
        switch self {
        case .vad: return .vad
        case .diarization: return .diarization
        case .parakeetUnified, .parakeetEou, .parakeetTdt, .nemotron, .nemotronMultilingual, .cohere, .canary,
            .senseVoice, .paraformer, .whisper, .appleSpeech:
            return .speech
        }
    }
}

extension StreamerPool where Value == Streamer {
    /// A pool whose managers all come from `load`. The main manager loads at the first use.
    init(load: @escaping @Sendable () async throws -> Streamer) {
        let free: @Sendable (Streamer) async -> Void = { await $0.cleanup() }
        self.init(main: LazyPart(load: load, free: free), loadCopy: load, free: free)
    }
}

/// One interface over the streaming managers. Nemotron multilingual has its own API
/// (a language prompt, float samples) and does not conform to `StreamingAsrManager`.
enum Streamer: Sendable {
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

    /// Finish, and give the text with the word times of the model. `offset` is the time in
    /// seconds of the audio before the recording (the silence of a final pass). `audioMs`
    /// is the length of the recording.
    func finishTimed(offset: Double, audioMs: Int) async throws -> Timed {
        switch self {
        case .standard(let manager):
            if let nemotron = manager as? StreamingNemotronAsrManager {
                let result = try await nemotron.finishWithTokenTimings()
                return Self.timed(result.text, result.timings, offset: offset, audioMs: audioMs)
            }
            if let eou = manager as? StreamingEouAsrManager {
                // `finish` clears the tokens, so take them first. The last chunk that
                // `finish` decodes is silence, so it adds no tokens.
                let tokens = await eou.getRawTokenStrings()
                let startsMs = await eou.getTokenTimestampsMs()
                let text = try await eou.finish()
                guard tokens.count == startsMs.count else { return .untimed(text) }
                let words = Segments.words(
                    tokens: tokens, starts: startsMs.map { Double($0) / 1000 - offset }, durations: nil,
                    audioMs: audioMs)
                return Timed(text: text, timing: words.isEmpty ? .none : .words(words))
            }
            return .untimed(try await manager.finish())
        case .nemotronMultilingual(let manager):
            let result = try await manager.finishWithTokenTimings()
            return Self.timed(result.text, result.timings, offset: offset, audioMs: audioMs)
        }
    }

    /// Text with the word times from FluidAudio token times. `offset` (seconds) moves the
    /// times back to the start of the recording.
    static func timed(_ text: String, _ timings: [TokenTiming], offset: Double = 0, audioMs: Int) -> Timed {
        let words = Segments.words(
            tokens: timings.map(\.token), starts: timings.map { $0.startTime - offset },
            durations: timings.map { $0.endTime - $0.startTime }, audioMs: audioMs)
        return Timed(text: text, timing: words.isEmpty ? .none : .words(words))
    }

    /// Free the models of the manager.
    func cleanup() async {
        switch self {
        case .standard(let manager): await manager.cleanup()
        case .nemotronMultilingual(let manager): await manager.cleanup()
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
    enum Phase {
        case running(Streamer)
        /// The manager of the stream loads. The audio waits here and goes to the model
        /// with the first audio or the end of the stream that comes after the load.
        case waiting(language: String, held: HeldAudio)
    }

    var phase: Phase
    let model: String
    let tracker: PartialTracker
    /// The manager of the stream, from `pool`. It goes back to the pool when the stream ends.
    let part: LazyPart<Streamer>
    let pool: StreamerPool<Streamer>
}

/// One VAD session. It is a stream: it shares the session ids and the limit of streams.
private struct VadSession {
    let model: String
    let stream: VadStream
}

/// The `PreviewTail` of one stream. The partial callback and the end of the stream
/// run on different threads.
final class PartialTracker: @unchecked Sendable {
    private let lock = NSLock()
    private var tail = PreviewTail()

    /// The committed text and the tail to send for a partial, or nil when nothing changed.
    func update(_ text: String) -> (committed: String, tentative: String)? {
        lock.lock()
        defer { lock.unlock() }
        return tail.update(text, at: ProcessInfo.processInfo.systemUptime)
    }

    /// The text to send, all committed, at the end of the stream, or nil when it was sent.
    func finish(_ text: String) -> String? {
        lock.lock()
        defer { lock.unlock() }
        return tail.finish(text)
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
    private var vadSessions: [Int: VadSession] = [:]
    /// Open sessions whose `stream_end` arrived and did not run yet.
    private var ending: Set<Int> = []
    /// The managers of streams that end now. They go back to their pools soon.
    private var closing: [LazyPart<Streamer>] = []
    /// The most streams that are open at the same time, on all models together. The
    /// protocol asks for at least 3: a dictation, and the two sources of a live session.
    static let maxStreams = 8

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

    /// `list_models`: one `model_state` per model, from disk only. `reply` holds the extra
    /// fields of the `ok` (the catalog, when the request had no `models`).
    func listModels(_ models: [(id: String, spec: EngineSpec)], id: String?, reply: [String: Any] = [:]) async {
        for entry in models {
            registry[entry.id] = entry.spec
            out.send(
                "model_state", id: id,
                ["model": entry.id, "state": await state(of: entry.id, spec: entry.spec)])
        }
        out.ok(id: id, reply)
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
                throw EngineError(
                    "download finished but the files are incomplete", reason: "incomplete_download")
            }
        } catch {
            out.send(
                "model_state", id: id,
                [
                    "model": model, "state": "failed", "message": "\(error)",
                    "reason": (error as? EngineError)?.reason ?? "download_failed",
                ])
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
        case .parakeetTdt(.fluidMini):
            // FluidAudio cannot download this repo. `modelsExist` does not know its folder.
            guard let folder = store.ownFolder(spec) else { throw EngineError("the model has no folder") }
            try await FluidSpeechMini.download(to: folder) { reporter.report($0) }
            guard FluidSpeechMini.isComplete(in: folder) else {
                throw EngineError(
                    "download finished but the files are incomplete", reason: "incomplete_download")
            }
        case .parakeetTdt(let version):
            guard let repo = version.repo else { throw EngineError("the model has no FluidAudio repo") }
            // The v3 repo holds several encoders. The variant selects the int8 one.
            try await ModelHub.download(
                repo, to: store.root, variant: version == .v3 ? "int8" : nil,
                progressHandler: reporter.stage(base: 0, weight: 1))
            guard let folder = store.ownFolder(spec),
                AsrModels.modelsExist(at: folder, version: version.fluid)
            else {
                throw EngineError(
                    "download finished but the files are incomplete", reason: "incomplete_download")
            }
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
                .senseVoiceSmall, to: store.root, variant: SenseVoice.precision.rawValue,
                progressHandler: reporter.stage(base: 0, weight: 1))
            // The int8 encoder of earlier versions is not used any more.
            if let folder = store.ownFolder(spec) {
                try? FileManager.default.removeItem(at: folder.appendingPathComponent(SenseVoice.oldEncoder))
            }
        case .paraformer:
            try await ModelHub.download(
                .paraformerLargeZh, to: store.root,
                progressHandler: reporter.stage(base: 0, weight: 1))
        case .appleSpeech:
            guard #available(macOS 26, *) else { throw EngineError("Apple Speech needs macOS 26") }
            // The assets of the language of this Mac. Other languages download on first use.
            try await AppleSpeech.install(language: "auto") { reporter.report($0) }
        case .fluidVad:
            try await ModelHub.download(.vad, to: store.root, progressHandler: reporter.stage(base: 0, weight: 1))
        case .fluidDiarization:
            // The "offline" variant: segmentation, FBank, embedding, and PLDA for the VBx clustering.
            try await ModelHub.download(
                .diarizer, to: store.root, variant: "offline",
                progressHandler: reporter.stage(base: 0, weight: 1))
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
            await info.pool.give(info.part)
        }
        for (session, info) in vadSessions where info.model == model {
            vadSessions[session] = nil
        }
        guard let entry = loaded.removeValue(forKey: model) else { return false }
        switch entry {
        case .parakeetUnified(let unified):
            await unified.cleanup()
        case .parakeetTdt(let manager):
            await manager.cleanup()
        case .parakeetEou(let pool), .nemotron(let pool), .nemotronMultilingual(let pool):
            // A final pass that runs keeps its manager until it ends.
            await pool.close()
        case .cohere, .canary, .senseVoice, .paraformer, .whisper, .appleSpeech, .vad, .diarization:
            break  // Released with the last reference.
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
            out.send(
                "model_state", id: id,
                [
                    "model": model, "state": "failed", "message": "\(error)",
                    "reason": (error as? EngineError)?.reason ?? "load_failed",
                ])
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
            return .parakeetUnified(UnifiedModel(
                batch: batch, config: config, directory: store.unifiedDir, ctcDirectory: store.ctcDir,
                ctcComplete: { [store] in store.ctcComplete() }, out: out))
        case .parakeetEou:
            let dir = store.eouDir
            return .parakeetEou(try await loadedPool {
                let eou = StreamingEouAsrManager(chunkSize: .ms160)
                try await eou.loadModels(from: dir)
                return .standard(eou)
            })
        case .whisper(let variant):
            let config = WhisperKitConfig(
                downloadBase: store.root, modelFolder: store.whisperDir(variant).path,
                tokenizerFolder: store.root, verbose: false, logLevel: .none,
                load: true, download: false)
            return .whisper(try await WhisperKit(config))
        case .parakeetTdt(.fluidMini):
            // `AsrModels.load` takes the files from FluidAudio's own repo folder and downloads
            // them there. `loadLocal` reads this folder only, and never uses the network.
            let dir = try folder(spec)
            try FluidSpeechMini.linkJointDecision(in: dir)
            let models = try AsrModels.loadLocal(from: dir, version: TdtVersion.fluidMini.fluid)
            let manager = AsrManager()
            try await manager.loadModels(models)
            return .parakeetTdt(manager)
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
            let dir = try folder(spec)
            return .nemotron(try await loadedPool {
                let manager = StreamingNemotronAsrManager()
                try await manager.loadModels(from: dir)
                return .standard(manager)
            })
        case .nemotronMultilingual:
            let dir = try folder(spec)
            return .nemotronMultilingual(try await loadedPool {
                let manager = StreamingNemotronMultilingualAsrManager()
                try await manager.loadModels(from: dir)
                return .nemotronMultilingual(manager)
            })
        case .cohere:
            let dir = try folder(spec)
            let models = try await CoherePipeline.loadModels(encoderDir: dir, decoderDir: dir, vocabDir: dir)
            return .cohere(CoherePipeline(), models)
        case .canary:
            return .canary(CanaryManager(models: try CanaryModels.load(from: try folder(spec), precision: .int4)))
        case .senseVoice:
            return .senseVoice(try SenseVoiceModels.load(from: try folder(spec), precision: SenseVoice.precision))
        case .paraformer:
            return .paraformer(ParaformerManager(models: try ParaformerModels.load(from: try folder(spec))))
        case .appleSpeech:
            return .appleSpeech
        case .fluidVad:
            // `VadManager(modelDirectory:)` downloads into its own folder when a file is
            // missing. The model from this folder never uses the network.
            let configuration = MLModelConfiguration()
            configuration.computeUnits = VadConfig.default.computeUnits
            let url = try folder(spec).appendingPathComponent(ModelNames.VAD.sileroVadFile, isDirectory: true)
            return .vad(VadManager(config: .default, vadModel: try MLModel(contentsOf: url, configuration: configuration)))
        case .fluidDiarization:
            // FluidAudio finds the files in the repo folder under the models directory.
            return .diarization(try await OfflineDiarizerModels.load(from: store.root))
        }
    }

    /// The managers of a streaming model. The main manager loads now: the model does its
    /// final pass with it, and the final pass must not wait for a load.
    private func loadedPool(_ load: @escaping @Sendable () async throws -> Streamer) async throws
        -> StreamerPool<Streamer>
    {
        let pool = StreamerPool(load: load)
        _ = try await pool.main.get()
        return pool
    }

    /// A loaded model for a request of `task`. A model of another task gets an error.
    private func loadedModel(_ model: String, for task: ModelTask) throws -> LoadedModel {
        guard let entry = loaded[model] else { throw EngineError("model \(model) is not loaded") }
        guard entry.task == task else {
            throw EngineError("model \(model) is a \(entry.task.name) model, not a \(task.name) model")
        }
        return entry
    }

    private func folder(_ spec: EngineSpec) throws -> URL {
        guard let folder = store.ownFolder(spec) else { throw EngineError("the model has no folder") }
        return folder
    }

    // MARK: - Transcribe

    func transcribe(
        model: String, path: String, language: String, vocabulary: [String], segments: Bool, id: String?
    ) async throws {
        let entry = try loadedModel(model, for: .speech)
        let samples = try AudioConverter().resampleAudioFile(path: path)
        guard !samples.isEmpty else { throw EngineError("audio file has no samples") }

        let start = nowMs()
        let audioMs = samples.count * 1000 / AudioChunks.sampleRate
        // The whole recording as one piece.
        func whole(_ timed: Timed) -> (text: String, pieces: [AudioPiece]) {
            (timed.text, [AudioPiece(startMs: 0, endMs: audioMs, timed: timed)])
        }
        let result: (text: String, pieces: [AudioPiece])
        switch entry {
        case .parakeetUnified(let unified):
            result = whole(try await transcribeParakeet(
                unified, samples: samples, vocabulary: vocabulary, timed: segments))
        case .whisper(let kit):
            result = whole(try await transcribeWhisper(
                kit, samples: samples, language: language, vocabulary: vocabulary, timed: segments))
        case .parakeetTdt(let manager):
            var state = TdtDecoderState.make(decoderLayers: await manager.decoderLayerCount)
            // The language is a hint for the v3 family. The other versions ignore it.
            let asr = try await manager.transcribe(
                samples, decoderState: &state, language: Language(rawValue: language))
            result = whole(Streamer.timed(asr.text, asr.tokenTimings ?? [], audioMs: audioMs))
        case .nemotron(let pool):
            result = whole(try await transcribeStreaming(
                pool: pool, samples: samples, language: language, vocabulary: nil))
        case .nemotronMultilingual(let pool):
            result = whole(try await transcribeStreaming(
                pool: pool, samples: samples, language: language, vocabulary: vocabulary))
        case .cohere(let pipeline, let models):
            // Cohere needs a language. Detection is not possible, so "auto" means English.
            let cohere = try await pipeline.transcribeLong(
                audio: samples, models: models,
                language: CohereAsrConfig.Language(rawValue: language) ?? .english)
            result = whole(.untimed(cohere.text))
        case .canary(let manager):
            result = whole(.untimed(try await manager.transcribe(audio: samples)))
        case .senseVoice(let models):
            result = try await AudioChunks.transcribe(samples, maxSeconds: SenseVoice.maxSeconds) { chunk in
                try SenseVoice.transcribe(models: models, audio: chunk, language: SenseVoice.language(language))
            }
        case .paraformer(let manager):
            result = try await AudioChunks.transcribe(samples, maxSeconds: 25) { chunk in
                try await manager.transcribe(audio: chunk)
            }
        case .appleSpeech:
            guard #available(macOS 26, *) else { throw EngineError("Apple Speech needs macOS 26") }
            result = whole(try await AppleSpeech.transcribe(
                url: URL(fileURLWithPath: path), language: language, vocabulary: vocabulary,
                timed: segments, audioMs: audioMs))
        case .parakeetEou(let pool):
            result = whole(try await transcribeStreaming(
                pool: pool, samples: samples, language: language, vocabulary: nil))
        case .vad, .diarization:
            throw EngineError("model \(model) is a \(entry.task.name) model, not a speech model")
        }
        var fields: [String: Any] = [
            "text": result.text.trimmingCharacters(in: .whitespacesAndNewlines), "elapsed_ms": Int(nowMs() - start),
        ]
        if segments { fields["segments"] = Segments.json(Segments.of(result.pieces)) }
        out.send("final", id: id, fields)
    }

    /// One second of silence before and after the audio of a streaming final pass. The
    /// cache-aware decoders gave no text for many short dictations ("yes", "okay")
    /// without it. In the benchmark it cut empty results and lowered the WER of every
    /// streaming model. The live preview gets none: the preview of Parakeet Unified got
    /// worse with lead-in silence.
    static let streamSilence = [Float](repeating: 0, count: 16_000)

    /// Final pass with a streaming model: feed the whole recording through a manager of
    /// the model. Streams of the model that are open keep their managers. `vocabulary`
    /// biases Nemotron multilingual, and nil leaves the manager as it is.
    private func transcribeStreaming(
        pool: StreamerPool<Streamer>, samples: [Float], language: String, vocabulary: [String]?
    ) async throws -> Timed {
        // The live preview of the same dictation usually ends just before its final pass,
        // but its `stream_end` can run after this request. Wait a little for its manager,
        // so that the final pass does not load a copy of the model.
        let deadline = nowMs() + 5000
        while await !pool.mainIsFree, nowMs() < deadline,
            closing.contains(where: { $0 === pool.main })
                || sessions.contains(where: { ending.contains($0.key) && $0.value.part === pool.main })
        {
            try await Task.sleep(nanoseconds: 20_000_000)
        }
        let part = try await pool.take()
        do {
            let streamer = try await part.get()
            if let vocabulary, case .nemotronMultilingual(let manager) = streamer {
                // The words bias the decoder. An empty list turns the bias off.
                await manager.setCustomVocabulary(vocabulary.map { CustomVocabularyTerm(text: $0) })
            }
            try await streamer.begin(language: language, onPartial: { _ in })
            try await streamer.feed(Self.streamSilence + samples + Self.streamSilence)
            let timed = try await streamer.finishTimed(
                offset: Double(Self.streamSilence.count) / Double(AudioChunks.sampleRate),
                audioMs: samples.count * 1000 / AudioChunks.sampleRate)
            await pool.give(part)
            return timed
        } catch {
            await pool.give(part)
            throw error
        }
    }

    /// Final pass with Parakeet Unified. With `timed`, or with dictionary words, the pass
    /// also gives the token times.
    private func transcribeParakeet(
        _ unified: UnifiedModel, samples: [Float], vocabulary: [String], timed: Bool
    ) async throws -> Timed {
        let audioMs = samples.count * 1000 / AudioChunks.sampleRate
        var booster: VocabularyBooster?
        if !vocabulary.isEmpty {
            do { booster = try await unified.vocabulary.get(vocabulary) }
            catch { out.log("warn", "vocabulary ignored: \(error)") }
        }
        guard timed || booster != nil else { return .untimed(try await unified.batch.transcribe(samples)) }
        let result = try await unified.batch.transcribeWithTimings(samples)
        var text = result.text
        if let booster {
            text = await booster.rescore(text: result.text, tokenTimings: result.tokenTimings, samples: samples)
        }
        var timing = Streamer.timed(result.text, result.tokenTimings, audioMs: audioMs).timing
        // Dictionary words can change the text. The times then stay with the words that
        // they replace, or are lost when the number of words changed.
        if text != result.text, case .words(let words) = timing {
            timing = Segments.retext(words, to: text).map(Timing.words) ?? .none
        }
        return Timed(text: text, timing: timing)
    }

    /// Load optional parts before a dictation. Failure leaves lazy loading available.
    func prepare(model: String, preview: Bool, vocabulary: [String], id: String?) async throws {
        let entry = try loadedModel(model, for: .speech)
        func report(_ part: String, _ state: String) {
            out.send("part_state", id: id, ["model": model, "part": part, "state": state])
        }
        // The two loads do not run predictions or change stream state.
        await withTaskGroup(of: Void.self) { group in
            if preview {
                group.addTask {
                    report("preview", "preparing")
                    do {
                        _ = try await entry.streamers?.main.get()
                        report("preview", "ready")
                    } catch {
                        report("preview", "failed")
                        self.out.log("warn", "preview preparation of \(model) failed: \(error)")
                    }
                }
            }
            if !vocabulary.isEmpty {
                group.addTask {
                    report("dictionary", "preparing")
                    do {
                        if case .parakeetUnified(let unified) = entry {
                            _ = try await unified.vocabulary.get(vocabulary)
                        }
                        report("dictionary", "ready")
                    } catch {
                        report("dictionary", "failed")
                        self.out.log("warn", "dictionary preparation of \(model) failed: \(error)")
                    }
                }
            }
        }
        out.ok(id: id)
    }

    /// Final pass with Whisper. With `timed`, WhisperKit also gives word times.
    private func transcribeWhisper(
        _ kit: WhisperKit, samples: [Float], language: String, vocabulary: [String], timed: Bool
    ) async throws -> Timed {
        let detect = language.isEmpty || language == "auto"
        var options = DecodingOptions(
            language: detect ? nil : language, detectLanguage: detect)
        // WhisperKit skips the last `windowClipTime` seconds (1 s) to stop "Thank you."
        // hallucinations after the speech. A recording of 1 s or less then gives no text at
        // all, so the clip is at most half of the recording.
        let seconds = Float(samples.count) / Float(WhisperKit.sampleRate)
        options.windowClipTime = min(options.windowClipTime, seconds / 2)
        // A first token below this log probability starts the temperature fallback, and
        // sampling near temperature 1 can end with no text. OpenAI's Whisper has no such
        // check, and clear speech failed it.
        options.firstTokenLogProbThreshold = nil
        // Dictionary words go in as prompt text. Whisper has no hard boosting.
        if !vocabulary.isEmpty, let tokenizer = kit.tokenizer {
            let prompt = " " + vocabulary.joined(separator: ", ")
            options.promptTokens = tokenizer.encode(text: prompt)
                .filter { $0 < tokenizer.specialTokens.specialTokenBegin }
            options.usePrefillPrompt = true
        }
        options.wordTimestamps = timed
        let results = try await kit.transcribe(audioArray: samples, decodeOptions: options)
        let text = results.map(\.text).joined(separator: " ").trimmingCharacters(in: .whitespacesAndNewlines)
        // Each word of Whisper starts with a space, except the parts of a word.
        let words = results.flatMap(\.segments).flatMap { $0.words ?? [] }
        let timedWords = Segments.words(
            tokens: words.map(\.word), starts: words.map { Double($0.start) },
            durations: words.map { Double($0.end - $0.start) }, audioMs: samples.count * 1000 / WhisperKit.sampleRate)
        return Timed(text: text, timing: timedWords.isEmpty ? .none : .words(timedWords))
    }

    // MARK: - Streaming

    /// Open a stream. Streams are independent (`concurrent_streams`): each stream has a
    /// manager of its own (see `StreamerPool`). A session id that is open already starts again.
    func streamStart(session: Int, model: String, language: String) async throws {
        let entry = try loadedModel(model, for: .speech)
        guard let pool = entry.streamers else { throw EngineError("model \(model) has no live preview") }
        try await reserve(session)
        let part = try await pool.take()
        let tracker = PartialTracker()
        var phase = StreamSession.Phase.waiting(language: language, held: HeldAudio())
        if let streamer = await part.loadedValue {
            do {
                try await streamer.begin(
                    language: language, onPartial: partialHandler(session: session, tracker: tracker))
            } catch {
                await pool.give(part)
                throw error
            }
            phase = .running(streamer)
        }
        // The model may have been unloaded while the stream started.
        guard loaded[model]?.streamers === pool else {
            await pool.give(part)
            throw EngineError("model \(model) is not loaded")
        }
        sessions[session] = StreamSession(phase: phase, model: model, tracker: tracker, part: part, pool: pool)
        // The first preview of a loaded model, or a second stream on it, loads a manager.
        // The load can take a minute after an update (Core ML compiles the model), and the
        // app waits for this reply while the user starts to speak. So reply now, and let
        // the audio wait.
        if case .waiting = phase { warm(part, model: model) }
    }

    /// Make room for the stream `session`. A session id that is open already starts again.
    /// The limit counts preview streams and VAD sessions together.
    private func reserve(_ session: Int) async throws {
        vadSessions[session] = nil
        if let old = sessions.removeValue(forKey: session) { await old.pool.give(old.part) }
        guard sessions.count + vadSessions.count < Self.maxStreams else {
            throw EngineError("\(Self.maxStreams) streams are open; end one first", code: "too_many_streams")
        }
    }

    /// Load a part in the background. A failure goes to the log. The next stream tries again.
    private func warm(_ part: LazyPart<Streamer>, model: String) {
        let out = self.out
        Task {
            do { _ = try await part.get() } catch is LazyPartError {} catch {
                out.log("warn", "live preview of \(model) did not load: \(error)")
            }
        }
    }

    /// The callback that turns text of the model into `partial` messages.
    private func partialHandler(session: Int, tracker: PartialTracker) -> @Sendable (String) -> Void {
        let out = self.out
        return { text in
            // The tentative tail is the words heard last (see `PreviewTail`).
            guard let split = tracker.update(text) else { return }
            out.send("partial", ["session": session, "committed": split.committed, "tentative": split.tentative])
        }
    }

    /// The streamer of a session that waited for its model, started and given the audio
    /// that came before. Nil while the model is not in memory yet.
    private func startWaiting(session: Int, info: StreamSession) async throws -> Streamer? {
        guard case .waiting(let language, var held) = info.phase,
            let streamer = await info.part.loadedValue
        else { return nil }
        try await streamer.begin(language: language, onPartial: partialHandler(session: session, tracker: info.tracker))
        let audio = held.take()
        if !audio.isEmpty { try await streamer.feed(audio) }
        return streamer
    }

    /// True when `info` is still the open stream of `session`.
    private func isOpen(_ session: Int, _ info: StreamSession) -> Bool {
        sessions[session]?.part === info.part
    }

    func streamAudio(session: Int, pcm: String) async throws {
        if let vad = vadSessions[session] {
            guard let data = Data(base64Encoded: pcm) else { throw EngineError("pcm is not base64") }
            sendSpeech(try await vad.stream.accept(Self.samples(fromInt16LE: data)), session: session)
            return
        }
        guard var info = sessions[session] else {
            throw EngineError("unknown stream session \(session)")
        }
        guard let data = Data(base64Encoded: pcm) else { throw EngineError("pcm is not base64") }
        let samples = Self.samples(fromInt16LE: data)
        switch info.phase {
        case .running(let streamer):
            try await streamer.feed(samples)
        case .waiting(let language, var held):
            // Put the new audio behind the audio that waits, then run both when the model is in memory.
            held.append(samples)
            info.phase = .waiting(language: language, held: held)
            guard let streamer = try await startWaiting(session: session, info: info) else {
                if isOpen(session, info) { sessions[session] = info }
                return
            }
            // The stream may have ended while the model started.
            guard isOpen(session, info) else { return }
            info.phase = .running(streamer)
            sessions[session] = info
        }
    }

    /// Mark a stream whose `stream_end` arrived, before it runs. A final pass that comes
    /// after it can then wait for the manager of the stream.
    func streamEnding(session: Int) {
        if sessions[session] != nil { ending.insert(session) }
    }

    /// End a stream. It leaves the open streams now, so its session id is free again. The
    /// task finishes the stream and gives its last text. The task can wait for the load of
    /// the manager of the stream, so the caller does not wait for it before the next
    /// stream request.
    func streamEnd(session: Int) throws -> Task<String, Error> {
        ending.remove(session)
        guard let info = sessions.removeValue(forKey: session) else {
            throw EngineError("unknown stream session \(session)")
        }
        // A final pass that comes now waits for this manager and does not load a copy.
        closing.append(info.part)
        return Task {
            do {
                let text = try await finishStream(session: session, info: info)
                await release(info)
                return text
            } catch {
                await release(info)
                throw error
            }
        }
    }

    private func finishStream(session: Int, info: StreamSession) async throws -> String {
        let streamer: Streamer
        switch info.phase {
        case .running(let running):
            streamer = running
        case .waiting:
            // The manager of the stream still loads. Wait for it, then decode the audio
            // that waited. The first load of a model can take a minute after an update.
            _ = try await info.part.get()
            guard let started = try await startWaiting(session: session, info: info) else { return "" }
            streamer = started
        }
        let text = try await streamer.finish()
        if let committed = info.tracker.finish(text) {
            out.send("partial", ["session": session, "committed": committed, "tentative": ""])
        }
        return text
    }

    /// Give the manager of an ended stream back to its pool.
    private func release(_ info: StreamSession) async {
        closing.removeAll { $0 === info.part }
        await info.pool.give(info.part)
    }

    // MARK: - VAD and diarization

    /// `vad`: the parts of a WAV file with speech. It uses a `VadStream` as a VAD session
    /// does, so a session that gets the same audio finds the same parts.
    func vad(model: String, path: String, id: String?) async throws {
        guard case .vad(let manager) = try loadedModel(model, for: .vad) else {
            throw EngineError("model \(model) is not a VAD model")
        }
        let samples = try AudioConverter().resampleAudioFile(path: path)
        let stream = await VadStream.silero(manager)
        var events = try await stream.accept(samples)
        events += try await stream.finish()
        let speech = SpeechEvent.parts(events).map { ["start_ms": $0.startMs, "end_ms": $0.endMs] }
        out.ok(id: id, ["speech": speech])
    }

    /// `vad_start`: open a VAD session. Its audio comes with `stream_audio`.
    func vadStart(session: Int, model: String) async throws {
        guard case .vad(let manager) = try loadedModel(model, for: .vad) else {
            throw EngineError("model \(model) is not a VAD model")
        }
        try await reserve(session)
        let stream = await VadStream.silero(manager)
        // The model may have been unloaded while the session started.
        guard loaded[model] != nil else { throw EngineError("model \(model) is not loaded") }
        vadSessions[session] = VadSession(model: model, stream: stream)
    }

    /// End a VAD session: send `speech_end` for speech that has not ended. False when
    /// `session` is not a VAD session.
    func endVadSession(_ session: Int) async throws -> Bool {
        guard let info = vadSessions.removeValue(forKey: session) else { return false }
        sendSpeech(try await info.stream.finish(), session: session)
        return true
    }

    private func sendSpeech(_ events: [SpeechEvent], session: Int) {
        for event in events {
            switch event {
            case .start(let startMs):
                out.send("speech_start", ["session": session, "start_ms": startMs])
            case .end(let startMs, let endMs):
                out.send("speech_end", ["session": session, "start_ms": startMs, "end_ms": endMs])
            }
        }
    }

    /// `diarize`: who speaks when in a WAV file. `speakers` is the number of speakers when
    /// the client knows it. The clustering of FluidAudio then makes exactly that many
    /// clusters, or fewer when the audio has fewer speech embeddings.
    func diarize(model: String, path: String, speakers: Int?, id: String?) async throws {
        guard case .diarization(let models) = try loadedModel(model, for: .diarization) else {
            throw EngineError("model \(model) is not a diarization model")
        }
        let samples = try AudioConverter().resampleAudioFile(path: path)
        let audioMs = samples.count * 1000 / AudioChunks.sampleRate
        var config = OfflineDiarizerConfig.default
        config.clustering.numSpeakers = speakers
        let manager = OfflineDiarizerManager(config: config)
        manager.initialize(models: models)
        var raw: [Diarization.RawTurn] = []
        if !samples.isEmpty {
            do {
                raw = try await manager.process(audio: samples).segments.map {
                    Diarization.RawTurn(
                        start: Double($0.startTimeSeconds), end: Double($0.endTimeSeconds), speaker: $0.speakerId)
                }
            } catch OfflineDiarizationError.noSpeechDetected {
                raw = []
            }
        }
        out.ok(id: id, Diarization.finish(raw, audioMs: audioMs).json)
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

    /// Transcribe each piece of `split` alone. Returns the joined text, and each piece
    /// with its time and text (timestamp level `none`).
    static func transcribe(
        _ samples: [Float], maxSeconds: Int, _ pass: ([Float]) async throws -> String
    ) async throws -> (text: String, pieces: [AudioPiece]) {
        var pieces: [AudioPiece] = []
        var offset = 0
        for chunk in split(samples, maxSeconds: maxSeconds) {
            let text = try await pass(chunk)
            let startMs = offset * 1000 / sampleRate
            offset += chunk.count
            pieces.append(AudioPiece(startMs: startMs, endMs: offset * 1000 / sampleRate, timed: .untimed(text)))
        }
        return (join(pieces.map(\.timed.text)), pieces)
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
