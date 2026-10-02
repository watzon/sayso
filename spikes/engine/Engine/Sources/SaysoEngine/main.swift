// SaysoEngine spike: NDJSON (stdin) -> NDJSON (stdout) speech sidecar.
// Hosts FluidAudio (Parakeet Unified batch + streaming) and WhisperKit.
import AVFoundation
import FluidAudio
import Foundation
import WhisperKit

// MARK: - Protocol output

// Keep the protocol channel clean: dup the real stdout for NDJSON, then point fd 1 at stderr
// so any stray print() from a dependency cannot corrupt the stream.
let protocolFD = dup(STDOUT_FILENO)
dup2(STDERR_FILENO, STDOUT_FILENO)
let outLock = NSLock()
let protocolOut = FileHandle(fileDescriptor: protocolFD)

func emit(_ id: String?, _ type: String, _ fields: [String: Any] = [:]) {
    var obj: [String: Any] = ["v": 1, "type": type]
    if let id { obj["id"] = id }
    for (k, v) in fields { obj[k] = v }
    guard let data = try? JSONSerialization.data(withJSONObject: obj, options: [.sortedKeys]) else { return }
    outLock.lock()
    defer { outLock.unlock() }
    protocolOut.write(data + Data([0x0A]))
}

func nowMs() -> Double { Double(DispatchTime.now().uptimeNanoseconds) / 1e6 }

// MARK: - Engine state

let modelsDir = URL(fileURLWithPath: ProcessInfo.processInfo.environment["SAYSO_MODELS_DIR"]
    ?? FileManager.default.currentDirectoryPath + "/.models", isDirectory: true)
let whisperVariant = "openai_whisper-large-v3-v20240930_626MB"

var batchMgr: UnifiedAsrManager?
var streamMgr: StreamingUnifiedAsrManager?
var whisper: WhisperKit?
var lastEngine: String?

/// Parse "70_2_2" into a UnifiedConfig. Default is the 320 ms tier.
func unifiedConfig(_ suffix: String) -> UnifiedConfig? {
    let p = suffix.split(separator: "_").compactMap { Int($0) }
    guard p.count == 3 else { return nil }
    return UnifiedConfig(leftFrames: p[0], chunkFrames: p[1], rightFrames: p[2])
}

struct EngineError: Error { let message: String }

func progressEmitter(_ id: String?, engine: String) -> ProgressHandler {
    return { p in
        var phase = "listing"
        var extra: [String: Any] = [:]
        switch p.phase {
        case .listing: phase = "listing"
        case .downloading(let done, let total):
            phase = "downloading"; extra = ["files_done": done, "files_total": total]
        case .compiling(let name): phase = "compiling"; extra = ["model": name]
        }
        var f: [String: Any] = ["engine": engine, "fraction": p.fractionCompleted, "phase": phase]
        for (k, v) in extra { f[k] = v }
        emit(id, "download_progress", f)
    }
}

// MARK: - Handlers

func loadModel(id: String?, req: [String: Any]) async throws {
    guard let engine = req["engine"] as? String else { throw EngineError(message: "missing engine") }
    let t0 = nowMs()
    emit(id, "model_state", ["engine": engine, "state": "downloading"])
    var downloadMs = 0.0
    switch engine {
    case "parakeet_unified_batch":
        let mgr = UnifiedAsrManager()
        let encoder = ModelNames.ParakeetUnified.offlineEncoderFile(precision: .int8)
        try await ModelHub.download(.parakeetUnified, to: modelsDir, variant: "offline",
                                    additionalModelNames: [encoder],
                                    progressHandler: progressEmitter(id, engine: engine))
        downloadMs = nowMs() - t0
        emit(id, "model_state", ["engine": engine, "state": "loading"])
        let t1 = nowMs()
        try await mgr.loadModels(to: modelsDir)
        batchMgr = mgr
        emit(id, "model_state", ["engine": engine, "state": "ready", "download_ms": downloadMs, "load_ms": nowMs() - t1])
    case "parakeet_unified_stream":
        let suffix = (req["model"] as? String) ?? "70_2_2"
        guard let cfg = unifiedConfig(suffix) else { throw EngineError(message: "bad tier \(suffix)") }
        let mgr = StreamingUnifiedAsrManager(config: cfg)
        let encoder = ModelNames.ParakeetUnified.streamingEncoderFile(precision: .int8, contextSuffix: cfg.contextSuffix)
        try await ModelHub.download(.parakeetUnified, to: modelsDir, variant: nil,
                                    additionalModelNames: [encoder],
                                    progressHandler: progressEmitter(id, engine: engine))
        downloadMs = nowMs() - t0
        emit(id, "model_state", ["engine": engine, "state": "loading", "tier": suffix, "latency_ms": cfg.latencyMs])
        let t1 = nowMs()
        try await mgr.loadModels(to: modelsDir)
        streamMgr = mgr
        emit(id, "model_state", ["engine": engine, "state": "ready", "tier": suffix,
                                 "download_ms": downloadMs, "load_ms": nowMs() - t1])
    case "whisper":
        let variant = (req["model"] as? String) ?? whisperVariant
        let folder = try await WhisperKit.download(variant: variant, downloadBase: modelsDir) { prog in
            emit(id, "download_progress", ["engine": engine, "fraction": prog.fractionCompleted, "phase": "downloading"])
        }
        downloadMs = nowMs() - t0
        emit(id, "model_state", ["engine": engine, "state": "loading"])
        let t1 = nowMs()
        let config = WhisperKitConfig(downloadBase: modelsDir, modelFolder: folder.path,
                                      verbose: false, logLevel: .none, load: true, download: false)
        whisper = try await WhisperKit(config)
        emit(id, "model_state", ["engine": engine, "state": "ready", "model": variant,
                                 "download_ms": downloadMs, "load_ms": nowMs() - t1])
    default:
        throw EngineError(message: "unknown engine \(engine)")
    }
    lastEngine = engine
}

func resolveEngine(_ req: [String: Any]) throws -> String {
    guard let e = (req["engine"] as? String) ?? lastEngine else { throw EngineError(message: "no model loaded") }
    return e
}

func loadSamples(_ req: [String: Any]) throws -> [Float] {
    guard let path = req["path"] as? String else { throw EngineError(message: "missing path") }
    return try AudioConverter().resampleAudioFile(path: path)
}

func transcribeFile(id: String?, req: [String: Any]) async throws {
    let engine = try resolveEngine(req)
    let t0 = nowMs()
    var text = ""
    switch engine {
    case "parakeet_unified_batch":
        guard let mgr = batchMgr else { throw EngineError(message: "batch not loaded") }
        text = try await mgr.transcribe(try loadSamples(req))
    case "whisper":
        guard let w = whisper else { throw EngineError(message: "whisper not loaded") }
        let samples = try loadSamples(req)
        let results = try await w.transcribe(audioArray: samples,
                                             decodeOptions: DecodingOptions(language: "en", detectLanguage: false))
        text = results.map(\.text).joined(separator: " ").trimmingCharacters(in: .whitespaces)
    default:
        throw EngineError(message: "transcribe_file unsupported for \(engine); use stream_file")
    }
    emit(id, "final", ["engine": engine, "text": text, "ms": nowMs() - t0])
}

func makeBuffer(_ chunk: ArraySlice<Float>) -> AVAudioPCMBuffer {
    let fmt = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: 16000, channels: 1, interleaved: false)!
    let buf = AVAudioPCMBuffer(pcmFormat: fmt, frameCapacity: AVAudioFrameCount(chunk.count))!
    buf.frameLength = AVAudioFrameCount(chunk.count)
    chunk.withUnsafeBufferPointer { buf.floatChannelData![0].update(from: $0.baseAddress!, count: chunk.count) }
    return buf
}

/// Feed a WAV in chunk_ms pieces. `realtime: true` sleeps one chunk duration between chunks.
func streamFile(id: String?, req: [String: Any]) async throws {
    guard let mgr = streamMgr else { throw EngineError(message: "stream engine not loaded") }
    let chunkMs = (req["chunk_ms"] as? Int) ?? 100
    let realtime = (req["realtime"] as? Bool) ?? false
    let samples = try loadSamples(req)
    let chunkLen = 16 * chunkMs
    try await mgr.reset()

    let streamStart = nowMs()
    nonisolated(unsafe) var fedMs = 0.0        // audio position of last fed chunk
    nonisolated(unsafe) var lastFeedAt = streamStart
    nonisolated(unsafe) var partials = 0
    await mgr.setPartialTranscriptCallback { text in
        partials += 1
        let t = nowMs()
        emit(id, "partial", ["text": text, "audio_ms": fedMs, "wall_ms": t - streamStart,
                             "since_chunk_ms": t - lastFeedAt])
    }
    var pos = 0
    while pos < samples.count {
        let end = min(pos + chunkLen, samples.count)
        fedMs = Double(end) / 16.0
        lastFeedAt = nowMs()
        try await mgr.appendAudio(makeBuffer(samples[pos..<end]))
        try await mgr.processBufferedAudio()
        pos = end
        if realtime {
            let target = streamStart + fedMs
            let wait = target - nowMs()
            if wait > 0 { try await Task.sleep(nanoseconds: UInt64(wait * 1e6)) }
        }
    }
    let tFinish = nowMs()
    let text = try await mgr.finish()
    emit(id, "final", ["engine": "parakeet_unified_stream", "text": text, "ms": nowMs() - tFinish,
                       "total_ms": nowMs() - streamStart, "audio_ms": Double(samples.count) / 16.0,
                       "partials": partials, "realtime": realtime])
}

// MARK: - Main loop

emit(nil, "log", ["message": "SaysoEngine ready", "models_dir": modelsDir.path, "pid": getpid()])

while let line = readLine(strippingNewline: true) {
    if line.trimmingCharacters(in: .whitespaces).isEmpty { continue }
    guard let data = line.data(using: .utf8),
          let req = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
          let type = req["type"] as? String
    else {
        emit(nil, "error", ["message": "invalid JSON line"])
        continue
    }
    let id = req["id"] as? String
    do {
        switch type {
        case "load_model": try await loadModel(id: id, req: req)
        case "transcribe_file": try await transcribeFile(id: id, req: req)
        case "stream_file": try await streamFile(id: id, req: req)
        case "mic_status":
            // Read-only probe (S4): never prompts. Shows whether this sidecar sees the parent's grant.
            let st = AVCaptureDevice.authorizationStatus(for: .audio)
            let names = [0: "notDetermined", 1: "restricted", 2: "denied", 3: "authorized"]
            emit(id, "log", ["message": "mic authorizationStatus=\(names[st.rawValue] ?? "?")",
                             "mic_status": names[st.rawValue] ?? "?", "pid": getpid(), "ppid": getppid()])
        case "shutdown":
            emit(id, "log", ["message": "bye"])
            exit(0)
        default: throw EngineError(message: "unknown request type \(type)")
        }
    } catch {
        emit(id, "error", ["message": "\(error)"])
    }
}
