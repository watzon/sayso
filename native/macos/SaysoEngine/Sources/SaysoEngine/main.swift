// SaysoEngine: speech sidecar for Sayso. NDJSON requests on stdin, NDJSON events on stdout.
// Hosts FluidAudio (Parakeet Unified, Parakeet EOU) and WhisperKit.
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

let engine = Engine(modelsDir: modelsDir, out: out)
out.log("info", "SaysoEngine \(engineVersion) ready, models in \(modelsDir.path), pid \(getpid())")

/// Handle one request that has a single terminal response (`ok`, `final`, `hello`, or `error`).
/// `model_state` and `download_progress` events stream out while a request runs.
func handle(_ request: Request) async {
    let id = request.id
    do {
        switch request.type {
        case "hello":
            out.send("hello", id: id, ["version": engineVersion, "protocol": protocolVersion])
        case "list_models":
            await engine.listModels(try request.models(), id: id)
        case "download":
            try await engine.download(
                model: try request.string("model"), spec: try request.engine(),
                sizeBytes: request.optionalInt64("size_bytes"), id: id)
        case "delete":
            let spec = request.optionalString("model") != nil ? try? request.engine() : nil
            try await engine.delete(model: try request.string("model"), spec: spec, id: id)
        case "load":
            try await engine.load(
                model: try request.string("model"), spec: try request.engine(), id: id)
        case "transcribe":
            try await engine.transcribe(
                model: try request.string("model"), path: try request.string("path"),
                language: request.optionalString("language") ?? "en",
                vocabulary: request.stringArray("vocabulary"), id: id)
        case "shutdown":
            out.ok(id: id)
            exit(0)
        default:
            throw EngineError("unknown request type \(request.type)")
        }
    } catch {
        out.error("\(error)", id: id)
    }
}

/// Streaming requests must run in arrival order, so they share one serial queue.
func handleStream(_ request: Request) async {
    let id = request.id
    do {
        let session = try request.int("session")
        switch request.type {
        case "stream_start":
            try await engine.streamStart(session: session, model: try request.string("model"))
            out.ok(id: id)
        case "stream_audio":
            try await engine.streamAudio(session: session, pcm: try request.string("pcm"))
        case "stream_end":
            let text = try await engine.streamEnd(session: session)
            out.ok(id: id, ["session": session, "text": text])
        default:
            throw EngineError("unknown request type \(request.type)")
        }
    } catch {
        out.error("\(error)", id: id)
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
    case "stream_start", "stream_audio", "stream_end":
        streamSink.yield(request)
    default:
        Task { await handle(request) }
    }
}
// stdin closed: the parent is gone.
exit(0)
