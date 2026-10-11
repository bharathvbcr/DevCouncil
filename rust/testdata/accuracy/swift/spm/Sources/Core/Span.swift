// P5 (added for the dead-policy card, second pass): a file-private helper in
// Core that shares its name with a private method of a DemoMain type. A
// `private` declaration cannot be imported, so `import Core` must not bind it.
private func forwardSpan(from start: Int, to end: Int) -> Int { end - start }

public func coreDrag(_ start: Int) -> Int { forwardSpan(from: start, to: start + 1) }
