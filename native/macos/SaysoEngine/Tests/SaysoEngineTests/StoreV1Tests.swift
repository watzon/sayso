import CryptoKit
import XCTest

@testable import SaysoEngine

/// The rules of the store with small files and no network. The conformance suite runs them
/// between two engine processes (`crates/sayso-engine-conformance/src/store.rs`).
final class StoreV1Tests: XCTestCase {
    private var dir: URL!

    override func setUpWithError() throws {
        dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws { try FileManager.default.removeItem(at: dir) }

    private func hex(_ bytes: some Sequence<UInt8>) -> String { bytes.map { String(format: "%02x", $0) }.joined() }

    /// An artifact in the JSON of `descriptors.json`, with the digest of "Layout".
    private func artifact(_ files: [(String, String)]) -> [String: Any] {
        let sorted = files.sorted { $0.0.utf8.lexicographicallyPrecedes($1.0.utf8) }
        let canonical = sorted.map { "\($0.0)\0\($0.1.utf8.count)\0\(hex(SHA256.hash(data: Data($0.1.utf8))))\n" }.joined()
        return [
            "source": ["kind": "hugging_face", "repo": "test/repo", "revision": "r"],
            "digest": hex(SHA256.hash(data: Data(canonical.utf8))),
            "files": sorted.map { ["path": $0.0, "size": $0.1.utf8.count, "sha256": hex(SHA256.hash(data: Data($0.1.utf8)))] },
        ]
    }

    /// `shared` is in the models `a` and `b`, and `only-a` and `only-b` in one each.
    private func data(revision: Int = 1) throws -> StoreDescriptors {
        func model(_ name: String, _ other: String) -> [String: Any] {
            [
                "model": name, "format": "test-1", "revision": revision,
                "parts": [["role": "model", "artifact": "shared"], ["role": "extra", "artifact": other]],
            ]
        }
        let json: [String: Any] = [
            "artifacts": [
                "shared": artifact([("main.bin", "shared bytes"), ("sub/vocab.txt", "words")]),
                "only-a": artifact([("a.bin", "bytes of a")]),
                "only-b": artifact([("b.bin", "bytes of b")]),
            ],
            "descriptors": [model("a", "only-a"), model("b", "only-b")],
        ]
        return try JSONDecoder().decode(StoreDescriptors.self, from: JSONSerialization.data(withJSONObject: json))
    }

    private let contents = [
        "main.bin": "shared bytes", "sub/vocab.txt": "words", "a.bin": "bytes of a", "b.bin": "bytes of b",
    ]

    private func put(_ artifact: StoreArtifact, _ files: URL) throws {
        for file in artifact.files {
            let path = files.appendingPathComponent(file.path)
            try FileManager.default.createDirectory(at: path.deletingLastPathComponent(), withIntermediateDirectories: true)
            try Data(contents[file.path]!.utf8).write(to: path)
        }
    }

    private func store() throws -> StoreV1 { try StoreV1(root: dir.appendingPathComponent("v1"), hook: nil) }

    private func download(_ store: StoreV1, _ data: StoreDescriptors, _ model: String) throws {
        let descriptor = try XCTUnwrap(data.find(model: model, format: "test-1"))
        try store.download(data, descriptor, fetch: { artifact, _, files in try self.put(artifact, files) }, waiting: {})
    }

    private func names(_ store: StoreV1, _ folder: String) throws -> [String] {
        try FileManager.default.contentsOfDirectory(atPath: store.root.appendingPathComponent(folder).path).sorted()
    }

    func testADownloadPublishesTheArtifactsAndTheManifest() throws {
        let (store, data) = (try store(), try data())
        let a = try XCTUnwrap(data.find(model: "a", format: "test-1"))
        XCTAssertFalse(try store.onDisk(a))
        XCTAssertNil(try store.load(a))

        try download(store, data, "a")

        XCTAssertTrue(try store.onDisk(a))
        XCTAssertEqual(try store.onDiskNow(a), true)
        XCTAssertEqual(try names(store, "staging"), [])
        let lease = try XCTUnwrap(try store.load(a))
        XCTAssertEqual(try String(contentsOf: lease.folders["model"]!.appendingPathComponent("sub/vocab.txt")), "words")
        XCTAssertFalse(FileManager.default.fileExists(atPath: lease.folders["model"]!.appendingPathComponent("owner").path))
        // A model on disk needs no fetch.
        try store.download(data, a, fetch: { _, _, _ in XCTFail("no fetch") }, waiting: {})
    }

    func testASharedArtifactStaysWhenTheOtherModelGoes() throws {
        let (store, data) = (try store(), try data())
        try download(store, data, "a")
        try download(store, data, "b")
        try store.delete(model: "a", format: "test-1")

        XCTAssertFalse(try store.onDisk(XCTUnwrap(data.find(model: "a", format: "test-1"))))
        XCTAssertTrue(try store.onDisk(XCTUnwrap(data.find(model: "b", format: "test-1"))))
        XCTAssertEqual(try names(store, "artifacts").count, 2)

        try store.delete(model: "b", format: "test-1")
        XCTAssertEqual(try names(store, "artifacts"), [])
        XCTAssertEqual(try names(store, "models"), [])
        XCTAssertEqual(try names(store, "trash"), [])
    }

    func testALoadedModelKeepsItsFilesUntilTheLeaseGoes() throws {
        let (store, data) = (try store(), try data())
        try download(store, data, "a")
        let a = try XCTUnwrap(data.find(model: "a", format: "test-1"))
        let lease = try XCTUnwrap(try store.load(a))

        try store.delete(model: "a", format: "test-1")

        XCTAssertFalse(try store.onDisk(a))
        XCTAssertNil(try store.load(a))
        XCTAssertTrue(FileManager.default.fileExists(atPath: lease.folders["model"]!.appendingPathComponent("main.bin").path))
        lease.release()
        try store.cleanup()
        XCTAssertEqual(try names(store, "artifacts"), [])
    }

    func testAManifestDoesNotGoBackToALowerRevision() throws {
        let store = try store()
        let (old, new) = (try data(revision: 1), try data(revision: 2))
        let (oldA, newA) = (try XCTUnwrap(old.find(model: "a", format: "test-1")), try XCTUnwrap(new.find(model: "a", format: "test-1")))
        try download(store, old, "a")
        XCTAssertFalse(try store.onDisk(newA), "a new descriptor wants a new revision")
        XCTAssertNil(try store.load(newA))

        try download(store, new, "a")
        XCTAssertTrue(try store.onDisk(oldA), "an older engine sees the model")

        // An older engine that stopped before the new manifest came.
        try store.download(old, oldA, fetch: { _, _, _ in XCTFail("no fetch") }, waiting: {})
        let manifest = try JSONSerialization.jsonObject(
            with: Data(contentsOf: store.root.appendingPathComponent("models/a/test-1.json"))) as? [String: Any]
        XCTAssertEqual(manifest?["revision"] as? Int, 2)
    }

    func testWrongFilesLeaveNothing() throws {
        let (store, data) = (try store(), try data())
        let a = try XCTUnwrap(data.find(model: "a", format: "test-1"))

        XCTAssertThrowsError(try store.download(data, a, fetch: { _, _, _ in throw EngineError("no network") }, waiting: {})) {
            guard case StoreError.fetch = $0 else { return XCTFail("\($0)") }
        }
        let wrongBytes: (StoreArtifact, URL, URL) throws -> Void = { artifact, _, files in
            for file in artifact.files {
                let path = files.appendingPathComponent(file.path)
                try FileManager.default.createDirectory(at: path.deletingLastPathComponent(), withIntermediateDirectories: true)
                try Data(repeating: 120, count: Int(file.size)).write(to: path)
            }
        }
        XCTAssertThrowsError(try store.download(data, a, fetch: wrongBytes, waiting: {})) {
            guard case StoreError.verify = $0 else { return XCTFail("\($0)") }
        }
        let withLink: (StoreArtifact, URL, URL) throws -> Void = { artifact, _, files in
            try self.put(artifact, files)
            try FileManager.default.createSymbolicLink(atPath: files.appendingPathComponent("link").path, withDestinationPath: "/etc/passwd")
        }
        XCTAssertThrowsError(try store.download(data, a, fetch: withLink, waiting: {})) {
            guard case StoreError.verify = $0 else { return XCTFail("\($0)") }
        }
        XCTAssertEqual(try names(store, "staging"), [])
        XCTAssertEqual(try names(store, "artifacts"), [])
        XCTAssertEqual(try names(store, "models"), [])
    }

    func testAManifestThatDoesNotParseStopsTheRemovalOfArtifacts() throws {
        let (store, data) = (try store(), try data())
        try download(store, data, "a")
        try Data("not json".utf8).write(to: store.root.appendingPathComponent("models/a/test-1.json"))
        try store.cleanup()
        XCTAssertEqual(try names(store, "artifacts").count, 2)
        XCTAssertFalse(try store.onDisk(XCTUnwrap(data.find(model: "a", format: "test-1"))))
    }

    func testCleanupKeepsADownloadThatRunsAndRemovesWhatACrashLeft() throws {
        let (store, data) = (try store(), try data())
        let stopped = store.root.appendingPathComponent("staging/\(data.artifacts["only-a"]!.digest).1.x/files")
        try FileManager.default.createDirectory(at: stopped, withIntermediateDirectories: true)
        let running = store.root.appendingPathComponent("staging/running.2.y")
        try FileManager.default.createDirectory(at: running, withIntermediateDirectories: true)
        let get = try StoreLock(path: store.root.appendingPathComponent("locks/running.get"))
        XCTAssertTrue(try get.tryLock(shared: false))
        let half = store.root.appendingPathComponent("models/a/.1.x.tmp")
        try FileManager.default.createDirectory(at: half.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data("{".utf8).write(to: half)

        try store.cleanup()

        XCTAssertEqual(try names(store, "staging"), ["running.2.y"])
        XCTAssertEqual(try names(store, "models"), [])
        get.close()
    }

    func testStrayFilesStopNothing() throws {
        let (store, data) = (try store(), try data())
        try download(store, data, "a")
        for folder in ["models", "artifacts", "staging", "trash"] {
            try Data("x".utf8).write(to: store.root.appendingPathComponent("\(folder)/.DS_Store"))
        }
        try store.delete(model: "a", format: "test-1")
        XCTAssertEqual(try names(store, "artifacts"), [".DS_Store"])
        XCTAssertEqual(try names(store, "models"), [".DS_Store"])
    }

    func testBadNamesAreRefused() throws {
        let store = try store()
        for bad in ["", "..", ".x", "a/b", "a\\b", "a:b"] {
            XCTAssertThrowsError(try store.delete(model: bad, format: "f"), bad)
        }
    }

    func testTheURLOfAFileIsPinnedAndFollowsTheEndpoint() throws {
        let source = StoreSource(kind: "hugging_face", repo: "o/r", revision: "abc", folder: "sub", url: nil)
        let file = StoreFile(path: "x y/z.bin", size: 1, sha256: "")
        XCTAssertEqual(
            try StoreDownload.url(of: file, in: source, endpoint: "http://127.0.0.1:9/").absoluteString,
            "http://127.0.0.1:9/o/r/resolve/abc/sub/x%20y/z.bin")
    }
}
