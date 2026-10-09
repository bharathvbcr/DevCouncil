import SwiftData
import Foundation

// C26 @Model SwiftData
@Model
final class Item {
    var title: String
    init(title: String) { self.title = title }
    func rename(_ t: String) { title = t }
}

func seed(context: ModelContext) {
    let item = Item(title: "a")              // -> Item
    item.rename("b")                         // -> Item.rename
    context.insert(item)                     // external
}
