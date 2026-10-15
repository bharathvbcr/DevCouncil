struct Parcel { func weight() -> Int { 1 } }
func loadItems() -> [Parcel] { [] }
struct Pouch { var items: [Parcel] }
struct Crate {
    var items: [Parcel]
    var pouch: Pouch?
    func total() { _ = items.map { $0.weight() } }
    var heavy: Int { items.filter { $0.weight() > 1 }.count }
    func fromFree() { let xs = loadItems(); _ = xs.map { $0.weight() } }
    func viaOptional(_ opt: Pouch?) { _ = (opt?.items ?? []).filter { $0.weight() > 1 } }
}
