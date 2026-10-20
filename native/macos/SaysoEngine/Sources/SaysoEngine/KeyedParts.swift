import Foundation

/// Builds for the same key share a load. Closing rejects all pending results.
actor KeyedParts<Key: Hashable & Sendable, Value: Sendable> {
    private var parts: [Key: LazyPart<Value>] = [:]
    private var closed = false
    private let build: @Sendable (Key) async throws -> Value

    init(build: @escaping @Sendable (Key) async throws -> Value) { self.build = build }

    func get(_ key: Key) async throws -> Value {
        guard !closed else { throw LazyPartError.closed }
        let part: LazyPart<Value>
        if let existing = parts[key] {
            part = existing
        } else {
            let build = self.build
            part = LazyPart(load: { try await build(key) }, free: { _ in })
            parts[key] = part
        }
        return try await part.get()
    }

    func close() async {
        closed = true
        let old = parts
        parts = [:]
        for part in old.values { await part.close() }
    }
}
