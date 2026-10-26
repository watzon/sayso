// The streaming managers of one model, so that streams and final passes run at the same time.
import Foundation

/// The streaming managers of one model.
///
/// A streaming manager holds the state of one audio stream, and FluidAudio cannot share
/// its Core ML models between two managers. So each stream and each streaming final pass
/// takes a manager of its own. The first user gets the main manager, which stays in memory
/// with the model. Each other user at the same time gets a copy that loads from the files
/// on disk on first use, and the copy is freed when its user gives it back.
///
/// After `close`, `take` throws. A part that a user still has is freed when it comes back.
actor StreamerPool<Value: Sendable> {
    /// The main manager. Its load can wait for the first use (see `LazyPart`).
    nonisolated let main: LazyPart<Value>
    private let loadCopy: @Sendable () async throws -> Value
    private let free: @Sendable (Value) async -> Void
    private var mainTaken = false
    private var closed = false

    init(
        main: LazyPart<Value>, loadCopy: @escaping @Sendable () async throws -> Value,
        free: @escaping @Sendable (Value) async -> Void
    ) {
        self.main = main
        self.loadCopy = loadCopy
        self.free = free
    }

    /// True when the main manager has no user.
    var mainIsFree: Bool { !mainTaken && !closed }

    /// A manager for one user: the main manager when it has no user, else a new copy.
    /// Give it back with `give` when the user is done.
    func take() throws -> LazyPart<Value> {
        guard !closed else { throw LazyPartError.closed }
        if !mainTaken {
            mainTaken = true
            return main
        }
        return LazyPart(load: loadCopy, free: free)
    }

    /// Take back a manager from `take`. A copy is freed. The main manager stays loaded.
    func give(_ part: LazyPart<Value>) async {
        guard part === main else {
            await part.close()
            return
        }
        mainTaken = false
        if closed { await main.close() }
    }

    /// Free the main manager when it has no user. Copies are freed when they come back.
    func close() async {
        closed = true
        if !mainTaken { await main.close() }
    }
}
