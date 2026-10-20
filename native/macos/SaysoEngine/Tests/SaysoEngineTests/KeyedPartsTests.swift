import XCTest
@testable import SaysoEngine

private actor BuildProbe {
    var keys: [[String]] = []
    private var gates: [CheckedContinuation<Void, Never>] = []
    private var open = false
    func build(_ key: [String]) async -> Int {
        keys.append(key)
        let value = keys.count
        if !open { await withCheckedContinuation { gates.append($0) } }
        return value
    }
    func waitForBuilds(_ count: Int) async {
        while keys.count < count { await Task.yield() }
    }
    func release() { open = true; for gate in gates { gate.resume() }; gates = [] }
}

final class KeyedPartsTests: XCTestCase {
    func testPreparationAndRealRequestShareBuildsByWordsAndAssets() async throws {
        let assetProbe = BuildProbe()
        let assets = LazyPart<Int>(load: { await assetProbe.build([]) }, free: { _ in })
        let builds = BuildProbe()
        let cache = KeyedParts<[String], Int> { words in
            _ = try await assets.get()
            return await builds.build(words)
        }
        async let prepared = cache.get(["Sayso"])
        async let real = cache.get(["Sayso"])
        async let changed = cache.get(["Parakeet"])
        await assetProbe.waitForBuilds(1)
        await assetProbe.release()
        await builds.waitForBuilds(2)
        await builds.release()
        let values = try await (prepared, real, changed)
        XCTAssertEqual(values.0, values.1)
        XCTAssertNotEqual(values.0, values.2)
        let assetKeys = await assetProbe.keys
        let keys = await builds.keys
        XCTAssertEqual(assetKeys.count, 1)
        XCTAssertEqual(Set(keys), Set([["Sayso"], ["Parakeet"]]))
        let cached = try await cache.get(["Sayso"])
        XCTAssertEqual(cached, values.0)
    }

    func testUnloadDuringPreparationRejectsTheResultAndFutureBuilds() async throws {
        let builds = BuildProbe()
        let cache = KeyedParts<[String], Int> { await builds.build($0) }
        let preparing = Task { try await cache.get(["Sayso"]) }
        await builds.waitForBuilds(1)
        await cache.close()
        await builds.release()
        do { _ = try await preparing.value; XCTFail("unloaded build returned") } catch is LazyPartError {}
        do { _ = try await cache.get(["new"]); XCTFail("unloaded cache built") } catch is LazyPartError {}
        let keys = await builds.keys
        XCTAssertEqual(keys, [["Sayso"]])
    }
}
