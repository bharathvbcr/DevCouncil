struct Gauge { var v: Int; static let zero = Gauge(v: 0); func bump() -> Int { v }; static func make(_ x: Int) -> Gauge { Gauge(v: x) } }
enum Kind { case one(Int); func area() -> Int { 1 } }
func freeDot() -> Int { let g: Gauge = .zero; return g.bump() }
func freeDotCall() -> Int { let g: Gauge = .make(1); return g.bump() }
func freeEnumCase() -> Int { let k: Kind = .one(2); return k.area() }
func freeEnumCaseNoArgs() -> Int { let k: Kind = Kind.one(2); return k.area() }
func multiDecl() -> Int { let a = Gauge(v: 1), g: Gauge = .zero; return a.bump() + g.bump() }
final class Host {
    func methodDot() -> Int { let g: Gauge = .zero; return g.bump() }
    func methodSecondLet() -> Int { let x = 1; let g: Gauge = .zero; return g.bump() + x }
}
