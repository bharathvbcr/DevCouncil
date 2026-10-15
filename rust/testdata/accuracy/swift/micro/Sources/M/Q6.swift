struct Rng {
    var seed: Int
    mutating func next() -> Int { seed += 1; return seed }
}
func roll() -> Int {
    var r = Rng(seed: 1)
    return r.next()
}
