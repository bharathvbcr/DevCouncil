struct FlEngine { func run() -> Int { 1 } }
struct FlOther { func run() -> Int { 2 } }
struct FlHolderA {
    var engine: FlEngine
    func go() -> Int { engine.run() }
}
struct FlHolderB {
    var engine = FlOther()
    func go() -> Int { engine.run() }
}
