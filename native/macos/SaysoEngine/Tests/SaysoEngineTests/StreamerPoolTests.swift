import XCTest

@testable import SaysoEngine

/// Counts the loads and frees of a pool.
private actor Counter {
    private(set) var loads = 0
    private(set) var freed: [Int] = []

    func load() -> Int {
        loads += 1
        return loads
    }

    func free(_ value: Int) { freed.append(value) }
}

final class StreamerPoolTests: XCTestCase {
    private func pool(_ counter: Counter) -> StreamerPool<Int> {
        StreamerPool(
            main: LazyPart(load: { await counter.load() }, free: { await counter.free($0) }),
            loadCopy: { await counter.load() }, free: { await counter.free($0) })
    }

    func testTheFirstUserGetsTheMainManager() async throws {
        let counter = Counter()
        let pool = pool(counter)
        let part = try await pool.take()
        XCTAssertTrue(part === pool.main)
        let free = await pool.mainIsFree
        XCTAssertFalse(free)
        await pool.give(part)
        let again = await pool.mainIsFree
        XCTAssertTrue(again)
        let freed = await counter.freed
        XCTAssertEqual(freed, [], "the main manager stays in memory")
    }

    func testUsersAtTheSameTimeGetCopiesThatAreFreedAfterUse() async throws {
        let counter = Counter()
        let pool = pool(counter)
        let first = try await pool.take()
        let second = try await pool.take()
        let third = try await pool.take()
        XCTAssertFalse(second === pool.main)
        XCTAssertFalse(second === third)
        let values = [try await first.get(), try await second.get(), try await third.get()]
        XCTAssertEqual(Set(values).count, 3, "each user has a manager of its own")
        await pool.give(second)
        await pool.give(third)
        let freed = await counter.freed
        XCTAssertEqual(Set(freed), Set(values.dropFirst()))
        let next = try await pool.take()
        XCTAssertFalse(next === pool.main, "the main manager still has its user")
    }

    func testCloseWaitsForTheUserOfTheMainManager() async throws {
        let counter = Counter()
        let pool = pool(counter)
        let part = try await pool.take()
        let value = try await part.get()
        await pool.close()
        var freed = await counter.freed
        XCTAssertEqual(freed, [], "a user keeps its manager until it ends")
        do {
            _ = try await pool.take()
            XCTFail("take after close must throw")
        } catch {}
        await pool.give(part)
        freed = await counter.freed
        XCTAssertEqual(freed, [value])
    }

    func testCloseFreesAMainManagerWithoutUser() async throws {
        let counter = Counter()
        let pool = pool(counter)
        let value = try await pool.main.get()
        await pool.close()
        let freed = await counter.freed
        XCTAssertEqual(freed, [value])
    }
}
