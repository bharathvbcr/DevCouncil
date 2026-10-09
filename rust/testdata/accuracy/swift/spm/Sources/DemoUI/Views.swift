import SwiftUI
import Observation
import Core

// C03 Observation macro model
@Observable
final class CounterModel {
    var count = 0
    func increment() { count += 1 }
}

// C04 ObservableObject view model
final class ListVM: ObservableObject {
    @Published var items: [String] = []
    func load() { items = ["a"] }
}

// C01 SwiftUI View + body composition; C02 @State/@Binding
struct RootView: View {
    @State private var model = CounterModel()                // C03 @State typed by initializer
    @StateObject private var vm = ListVM()                   // C04 @StateObject
    @Environment(\.dismiss) private var dismiss
    @State private var on = false

    var body: some View {
        VStack {
            HeaderView(title: "Demo")                          // C01 -> HeaderView
            ToggleRow(isOn: $on)                               // C02 -> ToggleRow
            Button("inc") { model.increment() }               // C03 -> CounterModel.increment
            Text(Person.make(named: "x").fullName())          // C24 cross-module -> Core Person.make, Person.fullName
            badge()                                            // C01 helper -> RootView.badge
        }
        .onAppear { vm.load() }                                // C04 -> ListVM.load
    }

    @ViewBuilder
    func badge() -> some View { Text("b") }
}

struct HeaderView: View {
    let title: String
    var body: some View { Text(title) }
}

struct ToggleRow: View {
    @Binding var isOn: Bool
    @ObservedObject var vm: ListVM
    init(isOn: Binding<Bool>) { _isOn = isOn; vm = ListVM() }
    var body: some View { Toggle("t", isOn: $isOn).onTapGesture { vm.load() } }   // -> ListVM.load
}

struct EnvReader: View {
    @Environment(CounterModel.self) private var model        // C03 env by type, no annotation
    var body: some View { Button("x") { model.increment() } } // -> CounterModel.increment
}

// C26 #Preview macro keeps RootView reachable
#Preview {
    RootView()
}
