import Foundation

// P-series: every private symbol below is LIVE (has a real caller) unless marked TRUE DEAD.
// `devmap dead` only considers private/fileprivate Swift symbols, so this file is where
// a missed edge becomes a false dead-code finding.

struct Meter: Equatable {
    var v: Int
    // P1 operator called by `a <+> b` below
    fileprivate static func <+> (l: Meter, r: Meter) -> Meter { Meter(v: l.v + r.v) }
    // P2 annotated local binding receiver: `let m: Meter = .zero` then m.privBump()
    fileprivate func privBump() -> Meter { Meter(v: v + 1) }
    static let zero = Meter(v: 0)
    // P3 optional-chained property typed only by annotation
    fileprivate func privOptional() -> Int { v }
    // P4 function value passed by name (no call parens)
    fileprivate static func privDouble(_ x: Int) -> Int { x * 2 }
    // P5 used only in string interpolation
    fileprivate func privLabel() -> String { "m" }
    // P11 TRUE DEAD
    private func privTrulyDead() {}
}

infix operator <+>: AdditionPrecedence

private protocol Pinger { func ping() -> Int }
extension Pinger {
    // P6 protocol-extension default calling a requirement (implicit self)
    fileprivate func pingTwice() -> Int { ping() + ping() }
}
private struct LocalPinger: Pinger {
    // P7 requirement implementation reached only through the protocol
    func ping() -> Int { 1 }
}

private final class Holder {
    var maybe: Meter?                         // typed by annotation only
    var cb: ((Int) -> Int)?
    // P8 property observer calling a private method
    var count = 0 { didSet { privObserved() } }
    fileprivate func privObserved() {}
    // P9 lazy var initializer calling a private method
    lazy var cached: Int = privCompute()
    fileprivate func privCompute() -> Int { 3 }
    // P10 private static helper reached through Self.
    fileprivate static func privStatic() -> Int { 4 }

    func drive() -> String {
        let a = Meter(v: 1), b = Meter(v: 2)
        let c = a <+> b                                     // P1
        let m: Meter = .zero
        let d = m.privBump()                                // P2
        let e = maybe?.privOptional() ?? 0                  // P3
        let f = [1, 2].map(Meter.privDouble)                // P4
        let g = "\(c.privLabel())"                          // P5
        let h = LocalPinger().pingTwice()                   // P6 -> pingTwice, and P7 ping via dispatch
        count += 1                                          // P8
        _ = cached                                          // P9
        let i = Self.privStatic()                           // P10
        return "\(d.v)\(e)\(f)\(g)\(h)\(i)"
    }
}

func useHolder() -> String { Holder().drive() }
