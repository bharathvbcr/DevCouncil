struct FlEngine { func run() -> Int { 1 } }
struct FlOther { func run() -> Int { 2 } }
func makeFlOther() -> FlOther { FlOther() }
struct FlHolderA {
    var engine: FlEngine
    func go() -> Int { engine.run() }
}
struct FlHolderB {
    var engine = FlOther()
    func go() -> Int { engine.run() }
}
struct FlHolderC {
    var engine = makeFlOther()
    func go() -> Int { engine.run() }
}
