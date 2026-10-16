extension Depot {
    typealias Stock = Lot
    func tally() { lots.forEach { _ = $0.mass() } }
}
