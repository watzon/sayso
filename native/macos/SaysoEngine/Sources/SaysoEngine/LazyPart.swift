// A part of a model that loads on first use, and audio that waits for it.
import Foundation

/// One part of a model that loads when something first needs it.
///
/// Concurrent callers share one load. A failed load leaves the part empty, so the next
/// caller tries again. `close` frees the value and does not wait for a load that runs:
/// that load frees its own result when it ends. After `close`, `get` throws.
actor LazyPart<Value: Sendable> {
    private let load: @Sendable () async throws -> Value
    private let free: @Sendable (Value) async -> Void
    private var value: Value?
    private var loading: Task<Value, Error>?
    private var closed = false

    init(
        load: @escaping @Sendable () async throws -> Value,
        free: @escaping @Sendable (Value) async -> Void
    ) {
        self.load = load
        self.free = free
    }

    var isLoaded: Bool { value != nil }

    /// The loaded value, or nil before it loads and while it loads.
    var loadedValue: Value? { value }

    /// The value. Loads it first when needed.
    func get() async throws -> Value {
        if let value { return value }
        guard !closed else { throw LazyPartError.closed }
        let task: Task<Value, Error>
        if let loading {
            task = loading
        } else {
            let load = self.load
            task = Task {
                do {
                    return try await self.finish(try await load())
                } catch {
                    self.fail()
                    throw error
                }
            }
            loading = task
        }
        return try await task.value
    }

    /// Free the value. A load that runs frees its result when it ends.
    func close() async {
        closed = true
        guard let value else { return }
        self.value = nil
        await free(value)
    }

    private func finish(_ loaded: Value) async throws -> Value {
        guard !closed else {
            await free(loaded)
            throw LazyPartError.closed
        }
        value = loaded
        loading = nil
        return loaded
    }

    private func fail() {
        loading = nil
    }
}

enum LazyPartError: Error, CustomStringConvertible {
    case closed
    var description: String { "the model was unloaded" }
}

/// Audio of a live stream that came before the model of the stream was in memory.
///
/// It holds at most `limit` samples (a minute at 16 kHz). Later audio is dropped,
/// so a model that never loads cannot grow it without end.
struct HeldAudio {
    static let limit = 16_000 * 60

    private var samples: [Float] = []

    var isEmpty: Bool { samples.isEmpty }
    var count: Int { samples.count }

    mutating func append(_ more: [Float]) {
        guard samples.count + more.count <= Self.limit else { return }
        samples.append(contentsOf: more)
    }

    /// Everything held, in order. The buffer is empty after this.
    mutating func take() -> [Float] {
        defer { samples = [] }
        return samples
    }
}
