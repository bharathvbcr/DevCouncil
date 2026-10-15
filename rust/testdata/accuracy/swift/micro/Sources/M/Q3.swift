protocol View {}
extension View {
    func padding() -> some View { self }
    func badge() -> some View { self }
}
struct Other { func badge() {} }
struct Base: View {}
struct Screen: View {
    var base: Base
    var body: some View { base.padding().badge() }
}
