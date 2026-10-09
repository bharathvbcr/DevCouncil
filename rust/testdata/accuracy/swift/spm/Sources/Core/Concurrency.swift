import Foundation

// C13 async/await, C14 actor and @MainActor, C12 closures
public actor Cache {
    private var storage: [String: Int] = [:]
    public init() {}
    public func get(_ key: String) -> Int? { storage[key] }
    public func put(_ key: String, _ v: Int) { storage[key] = v }
}

public enum Fetcher {
    public static func fetch(_ id: Int) async throws -> Int { id * 2 }
}

@MainActor
public final class Coordinator {
    let cache = Cache()                                  // C14 typed by initializer
    public init() {}

    public func start() async throws {
        let v = try await Fetcher.fetch(1)               // C13 -> Fetcher.fetch
        await cache.put("k", v)                          // C14 -> Cache.put
        _ = await cache.get("k")                         // C14 -> Cache.get
        async let w = compute(2)                         // C13 async let -> Coordinator.compute
        _ = await w
        Task { await self.refresh() }                    // C13 Task closure -> Coordinator.refresh
        retry(times: 2) { attempt in                     // C12 trailing closure -> Coordinator.retry
            self.log(attempt)                            // C12 inside closure -> Coordinator.log
        }
        let doubled = [1, 2].map { transform($0) }       // C12 -> transform (free fn)
        _ = doubled
    }

    func compute(_ x: Int) async -> Int { x }
    func refresh() async {}
    func retry(times: Int, _ body: (Int) -> Void) { for i in 0..<times { body(i) } }
    func log(_ i: Int) {}
}

public func transform(_ x: Int) -> Int { x + 1 }

// C30 closure-typed stored property called like a function must not bind to a same-named func
public struct Job {
    let onDone: () -> Void
    public init(onDone: @escaping () -> Void) { self.onDone = onDone }
    public func finish() { onDone() }                    // C30 -> NO edge to free func onDone below
}
public func onDone() {}                                  // TRUE DEAD; must not get a caller from Job.finish
