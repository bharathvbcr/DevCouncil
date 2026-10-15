struct Wrapper {
    private struct Rng {
        func next() -> Int { 0 }
    }
    func inner() -> Int { Rng().next() }
}
