struct Snap { var ok: Bool; func usage(for id: Int) -> Int { id } }
struct Probe { func snapshot() -> Snap { Snap(ok: true) } }
struct Store { func snapshot(_ p: Int) -> Snap { Snap(ok: false) } }
struct Svc {
    var root: String
    var limit: Int
    func scan() {}
}
final class AppDel { func openIt() {} }
func check() -> Bool { !Probe().snapshot().ok }
func scanIt() {
    Svc(
        root: "a",
        limit: 2
    ).scan()
}
func usageOf(_ store: Store, _ p: Int, _ id: Int) -> Int { store.snapshot(p).usage(for: id) }
func openAny(_ x: AnyObject) { (x as? AppDel)?.openIt() }
