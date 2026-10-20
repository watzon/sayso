import XCTest

@testable import SaysoEngine

/// Counts what a part does, and holds a load until the test lets it end.
private actor Probe {
    private(set) var loads = 0
    private(set) var freed: [Int] = []
    private var gate: CheckedContinuation<Void, Never>?
    private var open = false

    func started() -> Int {
        loads += 1
        return loads
    }

    /// Wait until `release`.
    func hold() async {
        if open { return }
        await withCheckedContinuation { gate = $0 }
    }

    func release() {
        open = true
        gate?.resume()
        gate = nil
    }

    func freed(_ value: Int) { freed.append(value) }
}

private struct Boom: Error {}

final class LazyPartTests: XCTestCase {
    private func part(_ probe: Probe, holding: Bool = false) -> LazyPart<Int> {
        LazyPart(
            load: {
                let value = await probe.started()
                if holding { await probe.hold() }
                return value
            },
            free: { await probe.freed($0) })
    }

    func testNothingLoadsBeforeTheFirstGet() async {
        let probe = Probe()
        let lazy = part(probe)
        let loaded = await lazy.isLoaded
        let loads = await probe.loads
        XCTAssertFalse(loaded)
        XCTAssertEqual(loads, 0)
    }

    func testTheLoadRunsOnceForSequentialCalls() async throws {
        let probe = Probe()
        let lazy = part(probe)
        let first = try await lazy.get()
        let second = try await lazy.get()
        let loads = await probe.loads
        XCTAssertEqual(first, 1)
        XCTAssertEqual(second, 1)
        XCTAssertEqual(loads, 1)
    }

    func testCallersAtTheSameTimeShareOneLoad() async throws {
        let probe = Probe()
        let lazy = part(probe, holding: true)
        async let a = lazy.get()
        async let b = lazy.get()
        async let c = lazy.get()
        // The load waits, so all three callers are waiting for it.
        try await Task.sleep(nanoseconds: 50_000_000)
        let loading = await lazy.isLoaded
        XCTAssertFalse(loading)
        await probe.release()
        let values = try await [a, b, c]
        let loads = await probe.loads
        XCTAssertEqual(values, [1, 1, 1])
        XCTAssertEqual(loads, 1)
    }

    func testAFailedLoadIsTriedAgain() async throws {
        let attempts = Probe()
        let lazy = LazyPart<Int>(
            load: {
                let attempt = await attempts.started()
                if attempt == 1 { throw Boom() }
                return attempt
            },
            free: { _ in })
        do {
            _ = try await lazy.get()
            XCTFail("the first load fails")
        } catch is Boom {}
        let value = try await lazy.get()
        XCTAssertEqual(value, 2)
    }

    func testCloseFreesTheLoadedValueOnce() async throws {
        let probe = Probe()
        let lazy = part(probe)
        _ = try await lazy.get()
        await lazy.close()
        await lazy.close()
        let freed = await probe.freed
        let loaded = await lazy.isLoaded
        XCTAssertEqual(freed, [1])
        XCTAssertFalse(loaded)
    }

    func testGetAfterCloseThrowsAndLoadsNothing() async throws {
        let probe = Probe()
        let lazy = part(probe)
        await lazy.close()
        do {
            _ = try await lazy.get()
            XCTFail("a closed part does not load")
        } catch is LazyPartError {}
        let loads = await probe.loads
        XCTAssertEqual(loads, 0)
    }

    func testCloseDuringALoadFreesTheResultWhenTheLoadEnds() async throws {
        let probe = Probe()
        let lazy = part(probe, holding: true)
        let waiting = Task { try await lazy.get() }
        try await Task.sleep(nanoseconds: 50_000_000)
        await lazy.close()  // Does not wait for the load.
        await probe.release()
        do {
            _ = try await waiting.value
            XCTFail("the caller of a closed part gets an error")
        } catch is LazyPartError {}
        let freed = await probe.freed
        let loaded = await lazy.isLoaded
        XCTAssertEqual(freed, [1])
        XCTAssertFalse(loaded)
    }
}

final class HeldAudioTests: XCTestCase {
    func testAudioComesBackInOrderAndTheBufferEmpties() {
        var held = HeldAudio()
        held.append([1, 2])
        held.append([3])
        XCTAssertEqual(held.count, 3)
        XCTAssertEqual(held.take(), [1, 2, 3])
        XCTAssertTrue(held.isEmpty)
    }

    func testAudioPastTheLimitIsDropped() {
        var held = HeldAudio()
        held.append([Float](repeating: 0.5, count: HeldAudio.limit))
        held.append([1])
        XCTAssertEqual(held.count, HeldAudio.limit)
    }
}
