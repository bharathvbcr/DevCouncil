import Foundation

protocol Renderable {
    func render() -> String
}

class Widget: BaseWidget, Renderable {
    var name: String = ""

    func render() -> String {
        return helper(name)
    }
}

func mainEntry() {
    let w = Widget()
    _ = w.render()
}
