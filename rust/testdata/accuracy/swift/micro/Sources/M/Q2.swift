struct Motor { func tick() {} }
func runSteps() {
    let steps: [(Motor) -> Void] = [{ $0.tick() }]
    _ = steps
}
func runCases() {
    let cases: [(name: String, apply: (Motor) -> Void)] = [("t", { $0.tick() })]
    _ = cases
}
