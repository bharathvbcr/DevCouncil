func b1() -> String { "" }
func b2() -> String { "" }
func b3() -> String { "" }
func b4() -> Int { 0 }
func b5() -> Int { 0 }
func b6() -> Bool { true }
func b7() -> Int { 0 }
func b8() -> String { "" }
func b9() -> Int { 0 }
func plusLeft() -> String { b1() + "x" }
func plusRight() -> String { "x" + b2() }
func plusBoth() -> String { b3() + b8() }
func minusRight() -> Int { 1 - b4() }
func mulRight() -> Int { 2 * b5() }
func andRight() -> Bool { true && b6() }
func cmpRight() -> Bool { 1 < b7() }
func nilCoalesce() -> Int { Optional<Int>.none ?? b9() }
struct Meter { var v: Int; static let zero = Meter(v: 0); func bump() -> Int { v } ; static func mk() -> Meter { zero } }
func annotatedDot() -> Int { let m: Meter = .zero; return m.bump() }
func annotatedCall() -> Int { let m: Meter = Meter.mk(); return m.bump() }
func annotatedOnly() -> Int { var m: Meter; m = Meter.zero; return m.bump() }
func ctorChain() -> Int { Meter(v: 1).bump() }
func paramTyped(_ m: Meter) -> Int { m.bump() }
func guardLet(_ o: Meter?) -> Int { guard let m = o else { return 0 }; return m.bump() }
func ifLet(_ o: Meter?) -> Int { if let o { return o.bump() }; return 0 }
func forIn(_ ms: [Meter]) -> Int { var t = 0; for m in ms { t += m.bump() }; return t }
