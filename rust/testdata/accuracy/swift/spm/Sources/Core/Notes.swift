import Foundation
import Combine

// C19 @objc / #selector / NotificationCenter; C15 Combine
public final class Watcher: NSObject, ObservableObject {
    @Published public var query: String = ""
    private var bag = Set<AnyCancellable>()

    public override init() {
        super.init()
        NotificationCenter.default.addObserver(
            self, selector: #selector(onNote(_:)),            // C19 -> Watcher.onNote (selector reference)
            name: .init("demo.note"), object: nil)
        $query.sink { [weak self] q in self?.handle(q) }      // C15 -> Watcher.handle
            .store(in: &bag)
        NotificationCenter.default.publisher(for: .init("demo.pub"))
            .sink { [weak self] _ in self?.onPublisher() }    // C15 -> Watcher.onPublisher
            .store(in: &bag)
    }

    @objc private func onNote(_ n: Notification) { handle("note") }
    func handle(_ q: String) {}
    func onPublisher() {}
}
