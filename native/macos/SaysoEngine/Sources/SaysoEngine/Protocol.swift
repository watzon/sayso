// NDJSON protocol v1: one JSON object per line on stdin (requests) and stdout (responses
// and events). Every message carries "v":1. See crates/sayso-engine-client/NOTES.md.
import Foundation

let protocolVersion = 1
let engineVersion = "0.1.0"

/// A failure that is reported to the client as `{"type":"error"}`. Never crashes the process.
struct EngineError: Error, CustomStringConvertible {
    let message: String
    init(_ message: String) { self.message = message }
    var description: String { message }
}

// MARK: - Output

/// Writes protocol lines to the real stdout. Thread safe.
///
/// `main.swift` points fd 1 at stderr before anything else runs, so a stray `print()`
/// in a dependency cannot corrupt the stream. This writer keeps a private duplicate
/// of the original stdout.
final class Output: @unchecked Sendable {
    private let fd: Int32
    private let lock = NSLock()

    init(fd: Int32) { self.fd = fd }

    /// Send one message. `fields` must hold JSON-compatible values only.
    func send(_ type: String, id: String? = nil, _ fields: [String: Any] = [:]) {
        var object: [String: Any] = ["v": protocolVersion, "type": type]
        if let id { object["id"] = id }
        for (key, value) in fields { object[key] = value }
        guard var data = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys]) else {
            return
        }
        data.append(0x0A)
        lock.lock()
        defer { lock.unlock() }
        data.withUnsafeBytes { raw in
            var offset = 0
            while offset < raw.count {
                let n = write(fd, raw.baseAddress! + offset, raw.count - offset)
                if n < 0 {
                    if errno == EINTR { continue }
                    return  // Parent closed the pipe. The stdin loop ends the process.
                }
                offset += n
            }
        }
    }

    func log(_ level: String, _ message: String) {
        send("log", ["level": level, "message": message])
    }

    func error(_ message: String, id: String? = nil) {
        send("error", id: id, ["message": message])
    }

    func ok(id: String?, _ fields: [String: Any] = [:]) {
        send("ok", id: id, fields)
    }
}

// MARK: - Requests

/// Which engine family runs a model. Mirrors `EngineKind` in `sayso-core`.
enum EngineSpec: Equatable {
    case parakeetUnified(streamingTier: String)
    case parakeetEou
    case whisper(variant: String)

    init(json: [String: Any]) throws {
        guard let kind = json["kind"] as? String else { throw EngineError("engine.kind is missing") }
        switch kind {
        case "parakeet_unified":
            self = .parakeetUnified(streamingTier: (json["streaming_tier"] as? String) ?? "70_2_2")
        case "parakeet_eou":
            self = .parakeetEou
        case "whisper":
            guard let variant = json["variant"] as? String, !variant.isEmpty else {
                throw EngineError("engine.variant is missing for kind whisper")
            }
            self = .whisper(variant: variant)
        default:
            throw EngineError("unknown engine kind \(kind)")
        }
    }
}

/// One parsed request line.
struct Request: @unchecked Sendable {
    let id: String?
    let type: String
    private let fields: [String: Any]

    init(line: String) throws {
        guard let data = line.data(using: .utf8),
            let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        else { throw EngineError("invalid JSON line") }
        id = object["id"] as? String
        guard let type = object["type"] as? String else { throw EngineError("missing type") }
        self.type = type
        fields = object
    }

    func string(_ key: String) throws -> String {
        guard let value = fields[key] as? String else { throw EngineError("missing string field \(key)") }
        return value
    }

    func optionalString(_ key: String) -> String? { fields[key] as? String }

    func int(_ key: String) throws -> Int {
        if let value = fields[key] as? Int { return value }
        if let value = fields[key] as? NSNumber { return value.intValue }
        throw EngineError("missing integer field \(key)")
    }

    func optionalInt64(_ key: String) -> Int64? { (fields[key] as? NSNumber)?.int64Value }

    func stringArray(_ key: String) -> [String] { (fields[key] as? [String]) ?? [] }

    func engine(key: String = "engine") throws -> EngineSpec {
        guard let json = fields[key] as? [String: Any] else { throw EngineError("missing object field \(key)") }
        return try EngineSpec(json: json)
    }

    /// The `models` array of `list_models`.
    func models() throws -> [(id: String, spec: EngineSpec)] {
        guard let list = fields["models"] as? [[String: Any]] else { throw EngineError("missing models array") }
        return try list.map { entry in
            guard let id = entry["id"] as? String else { throw EngineError("models[].id is missing") }
            guard let engine = entry["engine"] as? [String: Any] else {
                throw EngineError("models[].engine is missing")
            }
            return (id, try EngineSpec(json: engine))
        }
    }
}

// MARK: - Helpers

func nowMs() -> Double { Double(DispatchTime.now().uptimeNanoseconds) / 1e6 }
