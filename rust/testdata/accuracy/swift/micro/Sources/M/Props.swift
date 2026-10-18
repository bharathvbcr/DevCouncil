struct PlaybackState {
    var isPaused: Bool
    var count: Int
    func bump() -> Int { count + 1 }
    func viaSelf() -> Int { self.count }
    private var privTrulyDeadProp: Int { 7 }
    private var privReadProp: Int { 8 }
    func readPriv() -> Int { privReadProp }
}
extension PlaybackState {
    var doubled: Int { count * 2 }
}
struct PropTask { var isCompleted: Bool }
func pausedFlag(_ p: PlaybackState) -> Bool { p.isPaused }
func pausedLocal() -> Bool {
    let p = PlaybackState(isPaused: true, count: 1)
    return p.isPaused
}
func doneAny(_ tasks: [PropTask]) -> Bool { tasks.contains { $0.isCompleted } }
func doubledOf(_ p: PlaybackState) -> Int { p.doubled }
func makeUnknown() -> Unknown { Unknown() }
func untypedRead() -> Bool {
    let u = makeUnknown()
    return u.isPaused
}
struct IdA { let id: Int }
struct IdB { let id: Int }
func readA(_ a: IdA) -> Int { a.id }
struct Tally {
    var total: Int
    func make() -> Tally { Tally(total: 1) }
}
enum Axis { case size }
struct Gauge {
    var size: Int
    func pick() -> Axis { .size }
}
