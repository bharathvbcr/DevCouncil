import Foundation

// P7: the brief widens an executable target only after `@objc` / `#selector`.
// `onTap` is reached by selector and `onPing` by the Objective-C runtime
// alone; neither is a candidate. `register` is an ordinary uncalled member.
final class Tapper: NSObject {
    func register(_ center: NotificationCenter) {
        center.addObserver(self, selector: #selector(onTap(_:)), name: .init("tap"), object: nil)
    }

    @objc func onTap(_ note: Notification) {}

    @objc func onPing() {}
}
