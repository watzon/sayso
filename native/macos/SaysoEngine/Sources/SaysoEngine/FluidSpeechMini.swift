// Fluid Speech Mini: a Core ML model on Hugging Face that FluidAudio's `ModelHub` cannot
// fetch, because `ModelHub` only knows its own `Repo` list. This file has a small
// downloader for the one pinned revision. The model loads as Parakeet Ultra.
import CoreML
import FluidAudio
import Foundation

enum FluidSpeechMini {
    static let repoId = "FluidVoice-Labs/fluid-speech-mini-coreml"
    /// A pinned commit. A moving branch could change the files under an installed model.
    static let revision = "d81c3160dfe5a5e1f8f7267164a07d30617a3a1a"
    /// The folder name under the models directory.
    static let folderName = "fluid-speech-mini-coreml"

    static let bundles = [
        "Preprocessor.mlmodelc", "Encoder.mlmodelc", "Decoder.mlmodelc", "JointDecision.mlmodelc",
    ]
    static let vocabulary = "parakeet_vocab.json"
    /// Files outside the bundles. The README, the assets, and the examples are not needed.
    static let extraFiles = [vocabulary, "parakeet_v3_vocab.json", "config.json", "LICENSE", "NOTICE"]

    private static let attempts = 3

    /// One entry of the Hub tree listing.
    struct TreeEntry: Decodable, Equatable {
        let path: String
        let type: String
        let size: Int64?
    }

    /// The files the model needs, from a recursive tree listing.
    static func neededFiles(_ entries: [TreeEntry]) -> [TreeEntry] {
        entries.filter { entry in
            guard entry.type == "file" else { return false }
            return extraFiles.contains(entry.path)
                || bundles.contains { entry.path.hasPrefix($0 + "/") }
        }
    }

    /// True when the four bundles and the vocabulary are on disk. The check is local.
    /// `AsrModels.modelsExist` cannot do it: it adds FluidAudio's own repo folder name.
    static func isComplete(in folder: URL) -> Bool {
        let fm = FileManager.default
        let bundlesThere = bundles.allSatisfy {
            let bundle = folder.appendingPathComponent($0, isDirectory: true)
            return fm.fileExists(atPath: bundle.appendingPathComponent("coremldata.bin").path)
                && fm.fileExists(atPath: bundle.appendingPathComponent("model.mil").path)
                && fm.fileExists(atPath: bundle.appendingPathComponent("weights/weight.bin").path)
        }
        return bundlesThere && fm.fileExists(atPath: folder.appendingPathComponent(vocabulary).path)
    }

    /// FluidAudio's Ultra expects the joint model as `JointDecisionv3.mlmodelc`. This repo
    /// ships it as `JointDecision.mlmodelc` with the same outputs (`token_id`, `token_prob`,
    /// `duration`), so a link in the model folder gives the name FluidAudio wants.
    static func linkJointDecision(in folder: URL) throws {
        let fm = FileManager.default
        let link = folder.appendingPathComponent("JointDecisionv3.mlmodelc")
        if (try? fm.destinationOfSymbolicLink(atPath: link.path)) == "JointDecision.mlmodelc" { return }
        try? fm.removeItem(at: link)
        try fm.createSymbolicLink(atPath: link.path, withDestinationPath: "JointDecision.mlmodelc")
    }

    /// The models of an artifact in the model store (docs/sdk.md, findings 3 and 5). An artifact
    /// never changes, so it has no link to the name that FluidAudio wants, and `AsrModels.loadLocal`
    /// cannot read it. The compute units are those that `loadLocal` gives Parakeet Ultra.
    static func loadStored(from folder: URL) throws -> AsrModels {
        let configuration = AsrModels.defaultConfiguration()
        func model(_ name: String, units: MLComputeUnits) throws -> MLModel {
            let modelConfiguration = MLModelConfiguration()
            modelConfiguration.computeUnits = units
            modelConfiguration.allowLowPrecisionAccumulationOnGPU = configuration.allowLowPrecisionAccumulationOnGPU
            return try MLModel(contentsOf: folder.appendingPathComponent(name), configuration: modelConfiguration)
        }
        let data = try Data(contentsOf: folder.appendingPathComponent(vocabulary))
        guard let tokens = try JSONSerialization.jsonObject(with: data) as? [String: String] else {
            throw EngineError("the vocabulary of Fluid Speech Mini has an unknown form")
        }
        let words = tokens.reduce(into: [Int: String]()) { result, entry in
            if let id = Int(entry.key) { result[id] = entry.value }
        }
        return AsrModels(
            encoder: try model("Encoder.mlmodelc", units: configuration.computeUnits),
            preprocessor: try model("Preprocessor.mlmodelc", units: .cpuOnly),
            decoder: try model("Decoder.mlmodelc", units: configuration.computeUnits),
            joint: try model("JointDecision.mlmodelc", units: configuration.computeUnits),
            configuration: configuration, vocabulary: words, version: TdtVersion.fluidMini.fluid)
    }

    private static func url(api: Bool, path: String = "") -> URL {
        let encoded = path.split(separator: "/")
            .map { $0.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? String($0) }
            .joined(separator: "/")
        if api {
            return URL(
                string: "https://huggingface.co/api/models/\(repoId)/tree/\(revision)?recursive=true")!
        }
        return URL(string: "https://huggingface.co/\(repoId)/resolve/\(revision)/\(encoded)")!
    }

    /// Download every needed file into `folder`. `progress` gets a fraction in 0...1 of all bytes.
    static func download(to folder: URL, progress: @escaping @Sendable (Double) -> Void) async throws {
        let fm = FileManager.default
        try fm.createDirectory(at: folder, withIntermediateDirectories: true)

        let (data, response) = try await retrying { try await URLSession.shared.data(from: url(api: true)) }
        try requireOK(response)
        let files = neededFiles(try JSONDecoder().decode([TreeEntry].self, from: data))
        guard files.contains(where: { $0.path == vocabulary }) else {
            throw EngineError(
                "the file list of Fluid Speech Mini is incomplete", reason: "incomplete_download")
        }
        let total = max(1, files.reduce(Int64(0)) { $0 + ($1.size ?? 0) })

        var finished: Int64 = 0
        for file in files {
            try Task.checkCancellation()
            let size = file.size ?? 0
            let destination = folder.appendingPathComponent(file.path)
            // A file from an earlier try stays when its size is right.
            if size > 0, fileSize(destination) == size {
                finished += size
                progress(Double(finished) / Double(total))
                continue
            }
            let base = finished
            try await retrying {
                try await fetch(file.path, expectedSize: size, to: destination) { written in
                    progress(min(1, Double(base + written) / Double(total)))
                }
            }
            finished += size
        }
    }

    private static func fileSize(_ url: URL) -> Int64? {
        let attributes = try? FileManager.default.attributesOfItem(atPath: url.path)
        return (attributes?[.size] as? NSNumber)?.int64Value
    }

    private static func requireOK(_ response: URLResponse) throws {
        if let http = response as? HTTPURLResponse, !(200..<300).contains(http.statusCode) {
            throw EngineError("Hugging Face answered HTTP \(http.statusCode)")
        }
    }

    /// Run `body` up to `attempts` times, with a short pause between tries.
    private static func retrying<T>(_ body: () async throws -> T) async throws -> T {
        var lastError: Error?
        for attempt in 1...attempts {
            do {
                return try await body()
            } catch {
                if error is CancellationError || (error as? URLError)?.code == .cancelled { throw error }
                lastError = error
                if attempt < attempts { try await Task.sleep(nanoseconds: UInt64(attempt) * 1_000_000_000) }
            }
        }
        throw lastError ?? EngineError("download failed")
    }

    /// Download one file to a temporary path, check its size, then move it into place.
    private static func fetch(
        _ path: String, expectedSize: Int64, to destination: URL,
        written: @escaping @Sendable (Int64) -> Void
    ) async throws {
        let fm = FileManager.default
        try fm.createDirectory(
            at: destination.deletingLastPathComponent(), withIntermediateDirectories: true)
        let partial = destination.appendingPathExtension("partial")
        try? fm.removeItem(at: partial)
        defer { try? fm.removeItem(at: partial) }

        let loader = FileLoader(target: partial, onBytes: written)
        try await loader.run(url(api: false, path: path))
        if expectedSize > 0, fileSize(partial) != expectedSize {
            throw EngineError("\(path) has the wrong size", reason: "incomplete_download")
        }
        try? fm.removeItem(at: destination)
        try fm.moveItem(at: partial, to: destination)
    }
}

/// One URLSession download task with byte progress. It owns its session, so a finished
/// or failed task leaves nothing behind.
final class FileLoader: NSObject, URLSessionDownloadDelegate, @unchecked Sendable {
    private let target: URL
    private let onBytes: @Sendable (Int64) -> Void
    private let lock = NSLock()
    private var continuation: CheckedContinuation<Void, Error>?
    private var failure: Error?
    private var session: URLSession?
    private var task: URLSessionDownloadTask?

    init(target: URL, onBytes: @escaping @Sendable (Int64) -> Void) {
        self.target = target
        self.onBytes = onBytes
    }

    func run(_ url: URL) async throws {
        try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
                lock.lock()
                self.continuation = continuation
                let session = URLSession(configuration: .ephemeral, delegate: self, delegateQueue: nil)
                let task = session.downloadTask(with: url)
                self.session = session
                self.task = task
                lock.unlock()
                task.resume()
            }
        } onCancel: {
            lock.lock()
            let task = self.task
            lock.unlock()
            task?.cancel()
        }
    }

    func urlSession(
        _ session: URLSession, downloadTask: URLSessionDownloadTask, didWriteData bytesWritten: Int64,
        totalBytesWritten: Int64, totalBytesExpectedToWrite: Int64
    ) {
        onBytes(totalBytesWritten)
    }

    // The system deletes the temporary file when this method returns, so move it here.
    func urlSession(
        _ session: URLSession, downloadTask: URLSessionDownloadTask, didFinishDownloadingTo location: URL
    ) {
        if let http = downloadTask.response as? HTTPURLResponse, !(200..<300).contains(http.statusCode) {
            failure = EngineError("Hugging Face answered HTTP \(http.statusCode)")
            return
        }
        do {
            try? FileManager.default.removeItem(at: target)
            try FileManager.default.moveItem(at: location, to: target)
        } catch {
            failure = error
        }
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        lock.lock()
        let continuation = self.continuation
        self.continuation = nil
        lock.unlock()
        session.finishTasksAndInvalidate()
        if let error = error ?? failure {
            continuation?.resume(throwing: error)
        } else {
            continuation?.resume()
        }
    }
}
