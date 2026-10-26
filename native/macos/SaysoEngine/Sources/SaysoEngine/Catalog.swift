// The built-in catalog of the engine: the models that it runs on this Mac, by id
// (docs/engine-protocol.md, "Models"). The data is in Catalog.generated.swift.
import Foundation

struct Catalog {
    struct Entry {
        let id: String
        let spec: EngineSpec
        /// The entry as `list_models` sends it: the catalog fields without `min_macos`.
        let json: [String: Any]
    }

    /// A model id that was taken out of the catalog.
    struct Retired {
        let id: String
        /// What to use instead.
        let message: String
    }

    let models: [Entry]
    let retired: [Retired]

    /// Parse `json` (`{models, retired}`). `macOS` is the major version of macOS that runs
    /// the engine. A model that needs a newer macOS is left out.
    init(json: String, macOS: Int) throws {
        guard let data = json.data(using: .utf8),
            let root = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
            let list = root["models"] as? [[String: Any]], let retiredList = root["retired"] as? [[String: Any]]
        else { throw EngineError("the built-in catalog is not {models, retired}") }
        models = try list.compactMap { entry in
            guard let id = entry["id"] as? String, let engine = entry["engine"] as? [String: Any] else {
                throw EngineError("a model of the built-in catalog has no id or engine")
            }
            guard let minMacOS = (entry["min_macos"] as? NSNumber)?.intValue else {
                throw EngineError("model \(id) of the built-in catalog has no min_macos")
            }
            if minMacOS > macOS { return nil }
            var reply = entry
            reply["min_macos"] = nil
            return Entry(id: id, spec: try EngineSpec(json: engine), json: reply)
        }
        retired = try retiredList.map { entry in
            guard let id = entry["id"] as? String, let message = entry["message"] as? String else {
                throw EngineError("a retired entry of the built-in catalog has no id or message")
            }
            return Retired(id: id, message: message)
        }
    }

    /// The catalog of this build, for the macOS that runs it.
    static func builtIn() throws -> Catalog {
        try Catalog(json: BuiltInCatalog.json, macOS: ProcessInfo.processInfo.operatingSystemVersion.majorVersion)
    }

    /// The engine of a model that a request names without `engine`.
    func spec(of model: String) throws -> EngineSpec {
        if let entry = models.first(where: { $0.id == model }) { return entry.spec }
        if let old = retired.first(where: { $0.id == model }) {
            throw EngineError(
                "model \(model) was taken out of the catalog. \(old.message)", code: "model_retired")
        }
        throw EngineError("model \(model) is not in the catalog; send its engine object", code: "unknown_model")
    }

    /// The fields of the `ok` reply of `list_models` without `models`.
    var reply: [String: Any] {
        [
            "models": models.map(\.json),
            "retired": retired.map { ["id": $0.id, "message": $0.message] },
        ]
    }
}
