import Core
import SwiftUI

// P5: `forwardSpan` here is the struct's own method, called with an implicit
// `self` — ChronosFlow's ChronosDialCanvas. It was bound to Core's private one.
struct Dial {
    func updateDrag(_ start: Int) -> Int { forwardSpan(from: start, to: start + 2) + coreDrag(start) }
    private func forwardSpan(from start: Int, to end: Int) -> Int { end - start }
}

// P6: a member SwiftUI calls by requirement name; withheld from the widening.
struct DemoStyle: ViewModifier {
    func body(content: Content) -> some View { content }
}
