// SaysoEngine: speech sidecar for Sayso. NDJSON requests on stdin, NDJSON events on stdout.
// Hosts FluidAudio (Parakeet, Nemotron, Cohere, Canary, SenseVoice, Paraformer) and WhisperKit.
// It also runs the language model of macOS for AI enhancement (LanguageModel.swift).
import Foundation

// Keep the protocol channel clean. Duplicate the real stdout for NDJSON, then point fd 1 at
// stderr so a stray print() from a dependency cannot corrupt the stream.
let protocolFD = dup(STDOUT_FILENO)
dup2(STDERR_FILENO, STDOUT_FILENO)
signal(SIGPIPE, SIG_IGN)
setvbuf(stderr, nil, _IOLBF, 0)

let out = Output(fd: protocolFD)

guard let modelsPath = ProcessInfo.processInfo.environment["SAYSO_MODELS_DIR"], !modelsPath.isEmpty
else {
    out.error("SAYSO_MODELS_DIR is not set")
    exit(2)
}
let modelsDir = URL(fileURLWithPath: modelsPath, isDirectory: true)
try? FileManager.default.createDirectory(at: modelsDir, withIntermediateDirectories: true)

let catalog: Catalog
do {
    catalog = try Catalog.builtIn()
} catch {
    out.error("\(error)")
    exit(2)
}

// With `SAYSO_MODEL_STORE`, the models that have a descriptor are in that store, and a
// cleanup runs now (docs/sdk.md, "Clean up"). `SAYSO_STORE_TEST_HOOK` and
// `SAYSO_STORE_TEST_DESCRIPTORS` are for tests.
let environment = ProcessInfo.processInfo.environment
func setting(_ name: String) -> String? { environment[name].flatMap { $0.isEmpty ? nil : $0 } }
var sharedStore: StoreV1?
let storeDescriptors: StoreDescriptors
do {
    storeDescriptors = try StoreDescriptors.load(path: setting("SAYSO_STORE_TEST_DESCRIPTORS"))
    if let root = setting("SAYSO_MODEL_STORE") {
        let store = try StoreV1(
            root: URL(fileURLWithPath: root, isDirectory: true),
            hook: setting("SAYSO_STORE_TEST_HOOK").map { URL(fileURLWithPath: $0, isDirectory: true) })
        sharedStore = store
        Thread.detachNewThread {
            do { try store.cleanup() } catch { out.log("warn", "the cleanup of the model store failed: \(error)") }
        }
    }
} catch {
    out.error("cannot open the model store: \(error)")
    exit(2)
}

let engine = Engine(
    modelsDir: modelsDir, shared: sharedStore, descriptors: storeDescriptors,
    hfEndpoint: setting("SAYSO_HF_ENDPOINT") ?? "https://huggingface.co", out: out)
out.log("info", "SaysoEngine \(engineVersion) ready, models in \(modelsDir.path), pid \(getpid())")

/// Handle one request that has a single terminal response (`ok`, `final`, `hello`, or `error`).
/// `model_state` and `download_progress` events stream out while a request runs.
func handle(_ request: Request) async {
    let id = request.id
    do {
        switch request.type {
        case "hello":
            out.send(
                "hello", id: id,
                ["version": engineVersion, "protocol": protocolVersion, "features": engineFeatures])
        case "list_models":
            if let models = try request.models(catalog: catalog) {
                await engine.listModels(models, id: id)
            } else {
                await engine.listModels(catalog.models.map { ($0.id, $0.spec) }, id: id, reply: catalog.reply)
            }
        case "download":
            let model = try request.string("model")
            try await engine.download(
                model: model, spec: try request.engine(of: model, catalog: catalog),
                sizeBytes: request.optionalInt64("size_bytes"), id: id)
        case "delete":
            let spec = request.optionalString("model") != nil ? try? request.optionalEngine() : nil
            try await engine.delete(model: try request.string("model"), spec: spec, id: id)
        case "load":
            let model = try request.string("model")
            try await engine.load(model: model, spec: try request.engine(of: model, catalog: catalog), id: id)
        case "prepare":
            try await engine.prepare(model: try request.string("model"),
                preview: request.optionalBool("preview"), vocabulary: request.stringArray("vocabulary"), id: id)
        case "unload":
            try await engine.unload(model: try request.string("model"), id: id)
        case "transcribe":
            try await engine.transcribe(
                model: try request.string("model"), path: try request.string("path"),
                language: request.optionalString("language") ?? "en",
                vocabulary: request.stringArray("vocabulary"), segments: request.optionalBool("segments"), id: id)
        case "vad":
            try await engine.vad(model: try request.string("model"), path: try request.string("path"), id: id)
        case "diarize":
            try await engine.diarize(
                model: try request.string("model"), path: try request.string("path"),
                speakers: try request.optionalCount("speakers"), id: id)
        case "language_model":
            switch SystemModel.status() {
            case .success(let model): out.ok(id: id, ["available": true, "model": model])
            case .failure(let failure):
                out.ok(
                    id: id,
                    ["available": false, "message": failure.message, "reason": failure.reason])
            }
        case "generate":
            let reply = try await SystemModel.generate(
                instructions: try request.string("instructions"), prompt: try request.string("prompt"),
                temperature: request.optionalDouble("temperature"), timeoutMs: try request.int("timeout_ms"))
            out.ok(id: id, ["text": reply.text, "model": reply.model])
        case "shutdown":
            out.ok(id: id)
            exit(0)
        default:
            throw EngineError("unknown request type \(request.type)")
        }
    } catch let failure as LanguageModelError {
        out.error(failure.message, id: id, code: failure.code, reason: failure.reason)
    } catch {
        out.error("\(error)", id: id, code: (error as? EngineError)?.code)
    }
}

/// Streaming requests must run in arrival order, so they share one serial queue. A VAD
/// session is a stream too. The end of a stream replies when its text is ready (see
/// `Engine.streamEnd`).
func handleStream(_ request: Request) async {
    let id = request.id
    do {
        let session = try request.int("session")
        switch request.type {
        case "stream_start":
            try await engine.streamStart(
                session: session, model: try request.string("model"),
                language: request.optionalString("language") ?? "auto")
            out.ok(id: id)
        case "vad_start":
            try await engine.vadStart(session: session, model: try request.string("model"))
            out.ok(id: id)
        case "stream_audio":
            try await engine.streamAudio(session: session, pcm: try request.string("pcm"))
        case "stream_end":
            // A VAD session sends its last `speech_end` before the reply.
            if try await engine.endVadSession(session) {
                out.ok(id: id, ["session": session])
                return
            }
            let finishing = try await engine.streamEnd(session: session)
            // The end of a stream can wait for the load of its model. The stream requests
            // after it do not wait for that.
            Task {
                do { out.ok(id: id, ["session": session, "text": try await finishing.value]) } catch {
                    out.error("\(error)", id: id, code: (error as? EngineError)?.code)
                }
            }
        default:
            throw EngineError("unknown request type \(request.type)")
        }
    } catch {
        out.error("\(error)", id: id, code: (error as? EngineError)?.code)
    }
}

// Read stdin on its own thread so a blocking read never occupies a Swift concurrency thread.
let (lines, lineSink) = AsyncStream.makeStream(of: String.self)
Thread.detachNewThread {
    while let line = readLine(strippingNewline: true) { lineSink.yield(line) }
    lineSink.finish()
}
let (streamRequests, streamSink) = AsyncStream.makeStream(of: Request.self)
Task {
    for await request in streamRequests { await handleStream(request) }
}

for await line in lines {
    if line.trimmingCharacters(in: .whitespaces).isEmpty { continue }
    let request: Request
    do { request = try Request(line: line) } catch {
        out.error("\(error)")
        continue
    }
    switch request.type {
    case "stream_start", "vad_start", "stream_audio", "stream_end":
        if request.type == "stream_end", let session = try? request.int("session") {
            // Before the requests after it: a final pass that follows waits for this stream.
            await engine.streamEnding(session: session)
        }
        streamSink.yield(request)
    case "unload":
        // In arrival order: a `load` that follows must not find the model.
        await handle(request)
    default:
        Task { await handle(request) }
    }
}
// stdin closed: the parent is gone.
exit(0)
