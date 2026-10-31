// The model store: the rules of docs/sdk.md, "Model store", as in
// crates/sayso-core/src/model_store.rs. The layout, the lock order, and the names of the
// steps are the same, so two engines of any kind can share one store.
//
// `locks/meta` is shared to read manifests, and exclusive to change them or to decide what
// is garbage. `locks/<digest>.get` is exclusive while one engine downloads the artifact.
// `locks/<digest>.use` is shared while a model is loaded, and exclusive to delete. The order
// is `get`, `meta`, `use`, and locks of one kind go in the order of the digests.
//
// A lock can wait for as long as another engine downloads, and the test hook can wait for
// longer. So each call of `StoreV1` runs on a thread of `onStoreThread`, never on the
// `Engine` actor or on a thread of the Swift pool. Only `onDiskNow` does not wait.
import CryptoKit
import Foundation

/// The file formats of this engine (`engine_catalog::descriptors` in `sayso-core`).
enum StoreFormat {
    static let fluidAudio = "fluidaudio-coreml-1"
    static let whisperKit = "whisperkit-coreml-1"
    static let all = [fluidAudio, whisperKit]
}

extension EngineSpec {
    /// The format of the files of a model of this kind. Nil for a model that is not in the
    /// store: macOS manages the files of Apple Speech.
    var storeFormat: String? {
        switch self {
        case .parakeetUnified, .parakeetEou, .parakeetTdt, .nemotron, .nemotronMultilingual, .cohere, .canary,
            .senseVoice, .paraformer, .fluidVad, .fluidDiarization:
            return StoreFormat.fluidAudio
        case .whisper:
            return StoreFormat.whisperKit
        case .appleSpeech:
            return nil
        }
    }
}

// MARK: - Descriptors

struct StoreFile: Decodable {
    let path: String
    let size: Int64
    let sha256: String
}

/// Where the files of an artifact come from, pinned to a revision.
struct StoreSource: Decodable {
    let kind: String
    let repo: String?
    let revision: String?
    let folder: String?
    /// The URL of the source, for the kinds that are not Hugging Face.
    let url: String?
}

struct StoreArtifact: Decodable {
    let source: StoreSource
    let digest: String
    let files: [StoreFile]

    var size: Int64 { files.reduce(0) { $0 + $1.size } }
}

struct StorePart: Decodable {
    let role: String
    let artifact: String
}

struct StoreDescriptor: Decodable {
    let model: String
    let format: String
    let revision: Int
    let parts: [StorePart]
}

/// The data of `descriptors.json`.
struct StoreDescriptors: Decodable {
    let artifacts: [String: StoreArtifact]
    let descriptors: [StoreDescriptor]

    /// The descriptors of this engine: from `SAYSO_STORE_TEST_DESCRIPTORS`, else the built-in ones.
    static func load(path: String?) throws -> StoreDescriptors {
        let data = try path.map { try Data(contentsOf: URL(fileURLWithPath: $0)) } ?? Data(BuiltInDescriptors.json.utf8)
        return try JSONDecoder().decode(StoreDescriptors.self, from: data)
    }

    func find(model: String, format: String) -> StoreDescriptor? {
        descriptors.first { $0.model == model && $0.format == format }
    }

    /// The artifacts of `descriptor`, with their roles.
    func parts(of descriptor: StoreDescriptor) throws -> [(role: String, artifact: StoreArtifact)] {
        try descriptor.parts.map { part in
            guard let artifact = artifacts[part.artifact] else {
                throw StoreError.verify("the descriptor of \(descriptor.model) names the missing artifact \(part.artifact)")
            }
            return (part.role, artifact)
        }
    }
}

// MARK: - Errors and locks

enum StoreError: Error, CustomStringConvertible {
    case io(String)
    case name(String)
    /// The fetch function failed. A later download starts this artifact again.
    case fetch(String)
    /// The fetched files are not the files of the descriptor.
    case verify(String)

    var description: String {
        switch self {
        case .io(let message), .fetch(let message), .verify(let message): return message
        case .name(let name): return "\"\(name)\" is not a plain name"
        }
    }
}

/// Run a call of `StoreV1` on a thread that may wait for a long time.
func onStoreThread<T>(_ body: @escaping () throws -> T) async throws -> T {
    try await withCheckedThrowingContinuation { continuation in
        DispatchQueue.global().async { continuation.resume(with: Result { try body() }) }
    }
}

/// A `flock` on a file. It goes at `close`, and a process that stops lets it go.
final class StoreLock {
    private var fd: Int32

    init(path: URL) throws {
        fd = open(path.path, O_RDWR | O_CREAT | O_CLOEXEC, 0o644)
        guard fd >= 0 else { throw StoreError.io("cannot open the lock \(path.path): \(String(cString: strerror(errno)))") }
    }

    deinit { close() }

    func close() {
        if fd >= 0 { Darwin.close(fd) }
        fd = -1
    }

    /// Wait for the lock.
    func lock(shared: Bool) throws {
        while flock(fd, shared ? LOCK_SH : LOCK_EX) != 0 {
            guard errno == EINTR else { throw StoreError.io("cannot lock: \(String(cString: strerror(errno)))") }
        }
    }

    /// False when another holder has it now.
    func tryLock(shared: Bool) throws -> Bool {
        while flock(fd, (shared ? LOCK_SH : LOCK_EX) | LOCK_NB) != 0 {
            if errno == EWOULDBLOCK { return false }
            guard errno == EINTR else { throw StoreError.io("cannot lock: \(String(cString: strerror(errno)))") }
        }
        return true
    }
}

/// A model that is loaded: the folder of each role and the shared `use` locks. Keep it until
/// nothing reads the files.
final class StoreLease {
    let folders: [String: URL]
    private let locks: [StoreLock]

    init(folders: [String: URL], locks: [StoreLock]) {
        self.folders = folders
        self.locks = locks
    }

    func release() { locks.forEach { $0.close() } }
}

/// The files of a loaded model in the store. Each thing that reads them holds this. When the
/// last holder lets go, a cleanup removes the artifacts of a model that another engine has
/// deleted in the meantime.
final class StoreUse: @unchecked Sendable {
    private let lease: StoreLease
    let store: StoreV1

    init(lease: StoreLease, store: StoreV1) {
        self.lease = lease
        self.store = store
    }

    func folder(_ role: String) throws -> URL {
        guard let folder = lease.folders[role] else { throw EngineError("the manifest has no \(role) artifact") }
        return folder
    }

    deinit {
        lease.release()
        let store = self.store
        Thread.detachNewThread { try? store.cleanup() }
    }
}

// MARK: - The store

/// The store at one folder. A value of this type holds no state, so any thread can use it.
struct StoreV1: @unchecked Sendable {
    let root: URL
    private let hook: URL?
    private var fm: FileManager { .default }

    /// Open the store at `root`, and make its folders.
    init(root: URL, hook: URL?) throws {
        self.root = root
        self.hook = hook
        for folder in ["artifacts", "models", "locks", "staging", "trash"] {
            let path = root.appendingPathComponent(folder, isDirectory: true)
            do {
                try FileManager.default.createDirectory(at: path, withIntermediateDirectories: true)
            } catch {
                throw StoreError.io("cannot create \(path.path): \(error)")
            }
        }
    }

    /// A step of the rules. With the test hook, it writes `at-<step>` in the folder of the
    /// hook. With the file `hold-<step>` there, it then waits until that file is gone.
    func step(_ name: String) {
        guard let hook else { return }
        fm.createFile(atPath: hook.appendingPathComponent("at-\(name)").path, contents: Data())
        let hold = hook.appendingPathComponent("hold-\(name)").path
        while fm.fileExists(atPath: hold) { usleep(5_000) }
    }

    private func artifact(_ digest: String) -> URL { root.appendingPathComponent("artifacts/\(digest)", isDirectory: true) }

    private func hasArtifact(_ digest: String) -> Bool {
        var isDirectory: ObjCBool = false
        return fm.fileExists(atPath: artifact(digest).path, isDirectory: &isDirectory) && isDirectory.boolValue
    }

    private func lockFile(_ name: String) throws -> StoreLock { try StoreLock(path: root.appendingPathComponent("locks/\(name)")) }

    private func meta(shared: Bool) throws -> StoreLock {
        let lock = try lockFile("meta")
        try lock.lock(shared: shared)
        return lock
    }

    private func manifestPath(model: String, format: String) throws -> URL {
        for name in [model, format] {
            let plain = !name.isEmpty && !name.hasPrefix(".") && !name.contains { "/\\:".contains($0) }
            guard plain else { throw StoreError.name(name) }
        }
        return root.appendingPathComponent("models/\(model)/\(format).json")
    }

    /// Hold `meta` when you call this. A file that does not parse is no manifest.
    private func readManifest(model: String, format: String) throws -> Manifest? {
        let path = try manifestPath(model: model, format: format)
        guard let data = fm.contents(atPath: path.path) else {
            if fm.fileExists(atPath: path.path) { throw StoreError.io("cannot read \(path.path)") }
            return nil
        }
        return try? JSONDecoder().decode(Manifest.self, from: data)
    }

    /// Hold `meta` when you call this. The revision is enough and each artifact exists.
    private func complete(_ manifest: Manifest, _ descriptor: StoreDescriptor) -> Bool {
        manifest.revision >= descriptor.revision && manifest.parts.allSatisfy { hasArtifact($0.digest) }
    }

    /// True when the model is on disk: its manifest has the revision of the descriptor or a
    /// higher one, and each artifact exists.
    func onDisk(_ descriptor: StoreDescriptor) throws -> Bool {
        let meta = try self.meta(shared: true)
        defer { meta.close() }
        return try readManifest(model: descriptor.model, format: descriptor.format).map { complete($0, descriptor) } ?? false
    }

    /// `onDisk` without a wait: nil when another engine has `meta` now.
    func onDiskNow(_ descriptor: StoreDescriptor) throws -> Bool? {
        let meta = try lockFile("meta")
        defer { meta.close() }
        guard try meta.tryLock(shared: true) else { return nil }
        return try readManifest(model: descriptor.model, format: descriptor.format).map { complete($0, descriptor) } ?? false
    }

    /// "Load", steps 1 to 3: the folders of the model, locked for use. Nil when the model is
    /// not on disk. It never writes in the store, apart from the lock files.
    func load(_ descriptor: StoreDescriptor) throws -> StoreLease? {
        let meta = try self.meta(shared: true)
        defer { meta.close() }
        guard let manifest = try readManifest(model: descriptor.model, format: descriptor.format) else { return nil }
        step("load.manifest_read")
        guard manifest.revision >= descriptor.revision else { return nil }
        var locks: [StoreLock] = []
        for digest in Set(manifest.parts.map(\.digest)).sorted(by: Self.inDigestOrder) {
            let lock = try lockFile("\(digest).use")
            try lock.lock(shared: true)
            locks.append(lock)
        }
        step("load.use_locked")
        guard complete(manifest, descriptor) else { return nil }
        var folders: [String: URL] = [:]
        for part in manifest.parts { folders[part.role] = artifact(part.digest) }
        meta.close()
        step("load.ready")
        return StoreLease(folders: folders, locks: locks)
    }

    /// By the bytes of the name, as in the Rust engine.
    private static func inDigestOrder(_ a: String, _ b: String) -> Bool { a.utf8.lexicographicallyPrecedes(b.utf8) }

    /// "Download". `fetch` gets an artifact, an empty work folder, and the folder that must
    /// hold the files of the artifact when it returns. `waiting` runs when another engine
    /// downloads an artifact that this model needs.
    func download(
        _ data: StoreDescriptors, _ descriptor: StoreDescriptor,
        fetch: (StoreArtifact, URL, URL) throws -> Void, waiting: () -> Void
    ) throws {
        var artifacts: [StoreArtifact] = []
        for (_, artifact) in try data.parts(of: descriptor).sorted(by: { Self.inDigestOrder($0.artifact.digest, $1.artifact.digest) })
        where artifacts.last?.digest != artifact.digest {
            artifacts.append(artifact)
        }
        while true {
            // Step 1.
            let meta1 = try meta(shared: true)
            if let manifest = try readManifest(model: descriptor.model, format: descriptor.format), complete(manifest, descriptor) {
                meta1.close()
                return
            }
            let missing = artifacts.filter { !hasArtifact($0.digest) }
            meta1.close()
            step("download.manifest_read")
            // Step 2. The `get` locks stay until the artifacts are published.
            var gets: [StoreLock] = []
            defer { gets.forEach { $0.close() } }
            var staged: [(digest: String, stage: URL)] = []
            for artifact in missing {
                gets.append(try lockGet(artifact.digest, waiting: waiting))
                step("download.get_locked")
                let meta2 = try meta(shared: true)
                let exists = hasArtifact(artifact.digest)
                meta2.close()
                if exists { continue }
                do {
                    staged.append((artifact.digest, try stage(artifact, fetch: fetch)))
                } catch {
                    // A retry starts again, so a stage of this attempt is only in the way.
                    for (_, stage) in staged { try? fm.removeItem(at: stage) }
                    throw error
                }
            }
            // Step 3.
            let meta3 = try meta(shared: false)
            defer { meta3.close() }
            step("download.meta_locked")
            for (digest, stage) in staged {
                if !hasArtifact(digest) { try Self.rename(stage.appendingPathComponent("files"), artifact(digest)) }
                try? fm.removeItem(at: stage)
                step("download.artifact_published")
            }
            Self.syncFolder(root.appendingPathComponent("artifacts"))
            // A cleanup can take an artifact that no manifest named after step 1. Then this
            // goes back to step 1, with what it has published, and the `get` locks go in the
            // right order again.
            guard artifacts.allSatisfy({ hasArtifact($0.digest) }) else { continue }
            try writeManifest(descriptor, data)
            step("download.manifest_written")
            // A manifest with a higher revision stays, and it can name an artifact that a
            // power cut or a person took away.
            guard let manifest = try readManifest(model: descriptor.model, format: descriptor.format), complete(manifest, descriptor)
            else {
                throw StoreError.verify(
                    "the manifest of \(descriptor.model) has a higher revision and names a missing artifact; delete the model")
            }
            meta3.close()
            gets.forEach { $0.close() }
            step("download.done")
            return
        }
    }

    private func lockGet(_ digest: String, waiting: () -> Void) throws -> StoreLock {
        let lock = try lockFile("\(digest).get")
        if try !lock.tryLock(shared: false) {
            waiting()
            try lock.lock(shared: false)
        }
        return lock
    }

    /// Step 2 for one artifact: a staging folder, the `owner` file, the files, and the check
    /// against the descriptor. The caller holds the `get` lock. Returns the staging folder.
    private func stage(_ artifact: StoreArtifact, fetch: (StoreArtifact, URL, URL) throws -> Void) throws -> URL {
        let stage = root.appendingPathComponent("staging/\(artifact.digest).\(getpid()).\(Self.random())", isDirectory: true)
        let work = stage.appendingPathComponent("work", isDirectory: true)
        do {
            try fm.createDirectory(at: work, withIntermediateDirectories: true)
            try Data("\(getpid())\n".utf8).write(to: stage.appendingPathComponent("owner"))
        } catch {
            throw StoreError.io("cannot create \(stage.path): \(error)")
        }
        step("download.staged")
        let files = stage.appendingPathComponent("files", isDirectory: true)
        do {
            do { try fetch(artifact, work, files) } catch let error as StoreError { throw error } catch {
                throw StoreError.fetch("\(error)")
            }
            try Self.verify(files, artifact)
        } catch {
            try? fm.removeItem(at: stage)
            throw error
        }
        step("download.fetched")
        return stage
    }

    /// Hold `meta` exclusive when you call this. A manifest never goes back to a lower revision.
    private func writeManifest(_ descriptor: StoreDescriptor, _ data: StoreDescriptors) throws {
        if let existing = try readManifest(model: descriptor.model, format: descriptor.format),
            existing.revision > descriptor.revision
        {
            return
        }
        let manifest = Manifest(
            revision: descriptor.revision,
            parts: try data.parts(of: descriptor).map { Manifest.Part(role: $0.role, digest: $0.artifact.digest) })
        let path = try manifestPath(model: descriptor.model, format: descriptor.format)
        let folder = path.deletingLastPathComponent()
        let temp = folder.appendingPathComponent(".\(getpid()).\(Self.random()).tmp")
        do {
            try fm.createDirectory(at: folder, withIntermediateDirectories: true)
            let encoder = JSONEncoder()
            encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
            try encoder.encode(manifest).write(to: temp)
            try Self.syncFile(temp)
        } catch {
            throw StoreError.io("cannot write \(temp.path): \(error)")
        }
        try Self.rename(temp, path)
        Self.syncFolder(folder)
    }

    /// "Delete a model", then "Clean up". This never fails because of another engine: an
    /// engine that has the model loaded keeps it until it unloads.
    func delete(model: String, format: String) throws {
        let path = try manifestPath(model: model, format: format)
        let meta = try self.meta(shared: false)
        step("delete.meta_locked")
        if unlink(path.path) != 0, errno != ENOENT {
            meta.close()
            throw StoreError.io("cannot delete \(path.path): \(String(cString: strerror(errno)))")
        }
        step("delete.manifest_removed")
        meta.close()
        // The model is gone now. What a cleanup cannot do stays for the next one.
        try? cleanup()
    }

    /// "Clean up". Run it at the start of an engine and after each delete. What it cannot
    /// remove now (another engine has the artifact loaded) stays for the next cleanup.
    func cleanup() throws {
        do {
            let meta = try self.meta(shared: false)
            defer { meta.close() }
            step("cleanup.meta_locked")
            if let named = try namedDigests() {
                for digest in try folders(root.appendingPathComponent("artifacts")) where !named.contains(digest) {
                    let lock = try lockFile("\(digest).use")
                    defer { lock.close() }
                    guard try lock.tryLock(shared: false) else { continue }
                    step("cleanup.use_locked")
                    let trash = root.appendingPathComponent("trash/\(digest).\(Self.random())")
                    try? Self.rename(artifact(digest), trash)
                    step("cleanup.trashed")
                }
            }
        }
        step("cleanup.meta_released")
        for name in try names(in: root.appendingPathComponent("trash")) {
            try? fm.removeItem(at: root.appendingPathComponent("trash/\(name)"))
        }
        step("cleanup.trash_emptied")
        for name in try folders(root.appendingPathComponent("staging")) {
            let digest = name.split(separator: ".", omittingEmptySubsequences: false).first.map(String.init) ?? ""
            let lock = try lockFile("\(digest).get")
            defer { lock.close() }
            if try lock.tryLock(shared: false) {
                step("cleanup.staging_locked")
                try? fm.removeItem(at: root.appendingPathComponent("staging/\(name)"))
            }
        }
        step("cleanup.done")
    }

    /// Hold `meta` exclusive when you call this. The digests that the manifests name. It
    /// removes manifest files that a crash left half written, and the folder of a model
    /// without manifests. Nil when a manifest does not parse, so that it is not known what
    /// is garbage.
    private func namedDigests() throws -> Set<String>? {
        var named = Set<String>()
        // A file that is no folder (`.DS_Store`) is not a model.
        for model in try folders(root.appendingPathComponent("models")) {
            let folder = root.appendingPathComponent("models/\(model)", isDirectory: true)
            for file in try names(in: folder) {
                let path = folder.appendingPathComponent(file)
                if file.hasPrefix(".") {
                    try? fm.removeItem(at: path)
                    continue
                }
                guard let data = fm.contents(atPath: path.path) else { throw StoreError.io("cannot read \(path.path)") }
                guard let manifest = try? JSONDecoder().decode(Manifest.self, from: data) else { return nil }
                named.formUnion(manifest.parts.map(\.digest))
            }
            rmdir(folder.path)  // Only an empty folder goes.
        }
        return named
    }

    // MARK: Files

    private func names(in folder: URL) throws -> [String] {
        do { return try fm.contentsOfDirectory(atPath: folder.path) } catch {
            throw StoreError.io("cannot read \(folder.path): \(error)")
        }
    }

    /// The folders in `folder`. The store ignores other files that programs leave there.
    private func folders(_ folder: URL) throws -> [String] {
        try names(in: folder).filter {
            (try? fm.attributesOfItem(atPath: folder.appendingPathComponent($0).path)[.type] as? FileAttributeType) == .typeDirectory
        }
    }

    private static func random() -> String { UUID().uuidString.replacingOccurrences(of: "-", with: "").lowercased() }

    private static func rename(_ from: URL, _ to: URL) throws {
        guard Darwin.rename(from.path, to.path) == 0 else {
            throw StoreError.io("cannot move \(from.path): \(String(cString: strerror(errno)))")
        }
    }

    private static func syncFile(_ path: URL) throws {
        let fd = open(path.path, O_RDONLY | O_CLOEXEC)
        guard fd >= 0 else { throw StoreError.io("cannot read \(path.path)") }
        defer { close(fd) }
        if fcntl(fd, F_FULLFSYNC) != 0, fsync(fd) != 0 { throw StoreError.io("cannot write \(path.path)") }
    }

    /// Make a rename in `folder` last through a power cut, where the system can do that. A
    /// failure leaves the rename as good as the system made it.
    private static func syncFolder(_ folder: URL) { try? syncFile(folder) }

    /// Check that `folder` holds the files of `artifact` and nothing else.
    static func verify(_ folder: URL, _ artifact: StoreArtifact) throws {
        let found = try walk(folder, prefix: "").sorted()
        guard found == artifact.files.map(\.path).sorted() else {
            throw StoreError.verify("the files of \(artifact.digest) are not the files of its descriptor: \(found)")
        }
        for file in artifact.files {
            let path = folder.appendingPathComponent(file.path)
            let size = (try? FileManager.default.attributesOfItem(atPath: path.path)[.size] as? NSNumber)?.int64Value
            guard size == file.size else {
                throw StoreError.verify("\(file.path) has \(size ?? 0) bytes, not \(file.size)")
            }
            guard try hash(path) == file.sha256 else {
                throw StoreError.verify("\(file.path) does not have the hash of its descriptor")
            }
            // After a power cut, a published artifact must not have short files: only the
            // folder is checked at a load.
            try syncFile(path)
        }
    }

    /// The paths of the files under `folder`, in `/` form. A link is an error.
    private static func walk(_ folder: URL, prefix: String) throws -> [String] {
        let fm = FileManager.default
        guard let names = try? fm.contentsOfDirectory(atPath: folder.path) else {
            throw StoreError.io("cannot read \(folder.path)")
        }
        var found: [String] = []
        for name in names {
            let child = folder.appendingPathComponent(name)
            let kind = try? fm.attributesOfItem(atPath: child.path)[.type] as? FileAttributeType
            if kind == .typeDirectory {
                found += try walk(child, prefix: "\(prefix)\(name)/")
            } else if kind == .typeRegular {
                found.append("\(prefix)\(name)")
            } else {
                throw StoreError.verify("\(prefix)\(name) is not a file or a folder")
            }
        }
        return found
    }

    /// The SHA-256 of a file, in lowercase hex.
    static func hash(_ path: URL) throws -> String {
        guard let handle = try? FileHandle(forReadingFrom: path) else { throw StoreError.io("cannot read \(path.path)") }
        defer { try? handle.close() }
        var hasher = SHA256()
        while true {
            let more: Bool = try autoreleasepool {
                guard let chunk = try handle.read(upToCount: 1 << 20), !chunk.isEmpty else { return false }
                hasher.update(data: chunk)
                return true
            }
            if !more { return hasher.finalize().map { String(format: "%02x", $0) }.joined() }
        }
    }
}

/// What a manifest says: `models/<model id>/<format>.json`.
private struct Manifest: Codable {
    struct Part: Codable {
        let role: String
        let digest: String
    }
    let revision: Int
    let parts: [Part]
}

// MARK: - The download of an artifact

enum StoreDownload {
    private static let attempts = 3

    /// The URL of a file of an artifact. `endpoint` is `SAYSO_HF_ENDPOINT`, else Hugging Face.
    static func url(of file: StoreFile, in source: StoreSource, endpoint: String) throws -> URL {
        guard source.kind == "hugging_face", let repo = source.repo, let revision = source.revision else {
            throw StoreError.fetch("this engine cannot download a source of the kind \(source.kind)")
        }
        let parts = [source.folder, file.path].compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: "/")
        let encoded = parts.split(separator: "/")
            .map { $0.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? String($0) }
            .joined(separator: "/")
        let base = endpoint.hasSuffix("/") ? String(endpoint.dropLast()) : endpoint
        guard let url = URL(string: "\(base)/\(repo)/resolve/\(revision)/\(encoded)") else {
            throw StoreError.fetch("no URL for \(file.path)")
        }
        return url
    }

    /// Download each file of `artifact` into `files`, through `work`. `onBytes` gets the bytes
    /// of this artifact so far. It blocks, so it runs on a thread of `onStoreThread`.
    static func fetch(
        _ artifact: StoreArtifact, endpoint: String, work: URL, files: URL, onBytes: @escaping @Sendable (Int64) -> Void
    ) throws {
        let fm = FileManager.default
        var finished: Int64 = 0
        for file in artifact.files {
            let url = try url(of: file, in: artifact.source, endpoint: endpoint)
            let destination = files.appendingPathComponent(file.path)
            try fm.createDirectory(at: destination.deletingLastPathComponent(), withIntermediateDirectories: true)
            let partial = work.appendingPathComponent(UUID().uuidString)
            let base = finished
            var lastError: Error?
            for attempt in 1...attempts {
                do {
                    try load(url, to: partial) { onBytes(base + $0) }
                    lastError = nil
                    break
                } catch {
                    lastError = error
                    if attempt < attempts { sleep(UInt32(attempt)) }
                }
            }
            if let lastError { throw lastError }
            try fm.moveItem(at: partial, to: destination)
            finished += file.size
            onBytes(finished)
        }
    }

    /// One `FileLoader`, with a wait for its end.
    private static func load(_ url: URL, to target: URL, onBytes: @escaping @Sendable (Int64) -> Void) throws {
        let done = DispatchSemaphore(value: 0)
        let result = Mutex<Error?>(nil)
        Task.detached {
            do { try await FileLoader(target: target, onBytes: onBytes).run(url) } catch { result.value = error }
            done.signal()
        }
        done.wait()
        if let error = result.value { throw error }
    }
}

/// A value that two threads share.
private final class Mutex<Value>: @unchecked Sendable {
    private let lock = NSLock()
    private var stored: Value

    init(_ value: Value) { stored = value }

    var value: Value {
        get {
            lock.lock()
            defer { lock.unlock() }
            return stored
        }
        set {
            lock.lock()
            defer { lock.unlock() }
            stored = newValue
        }
    }
}
