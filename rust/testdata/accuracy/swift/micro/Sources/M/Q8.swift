struct Case { func apply(_ x: Int) -> Int { x } }
final class Joiner {
    var head = 0
    var out = 0
    func apply(_ open: (transform: Case, start: Int), _ x: Int) { out = head + open.transform.apply(x) }
}
