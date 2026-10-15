struct Cmd { func inverse() {} }
struct Hist { func pop() -> Cmd? { nil } }
struct Mgr { static let shared = Mgr(); func run() {} }
struct Client { func send(_ x: Int) {} }
@propertyWrapper struct Env {
    var wrappedValue: Client
    init(_ type: Client.Type) { wrappedValue = Client() }
}
func makePair() -> (Store, Svc) { (Store(), Svc(root: "", limit: 0)) }
extension Svc { func stash(_ x: Int) {} }
final class Runner {
    @Env(Client.self) private var client
    private let m = Mgr.shared
    var history = Hist()
    func makeMotor() -> Motor { Motor() }
    func t1() { let e = makeMotor(); e.tick() }
    func t2(_ store: Store) { let s = store.snapshot(1); _ = s.usage(for: 1) }
    func t3(_ x: Int) { let (store, svc) = makePair(); _ = store; svc.stash(x) }
    func t4() { m.run() }
    func t5(_ x: Int) { client.send(x) }
    func t6() { if let u = history.pop() { u.inverse() } }
}
