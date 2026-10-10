// C07 protocols, protocol extension default implementations, dispatch to conformers
public protocol Greeter {
    func greet() -> String
}

extension Greeter {
    public func hello() -> String { "hello, " + greet() }   // -> Greeter.greet requirement (dispatch: English.greet, French.greet)
}

public struct English: Greeter {
    public init() {}
    public func greet() -> String { "hi" }
}

public struct French: Greeter {
    public init() {}
    public func greet() -> String { "salut" }
}

public func greetAll() -> [String] {
    let e = English()
    let viaDefault = e.hello()                  // C07 -> Greeter.hello (protocol-extension default)
    let g: Greeter = French()
    let viaExistential = g.greet()              // C07 -> French.greet / English.greet (any conformer)
    return [viaDefault, viaExistential]
}

// C17 generics constrained to a protocol + `some`
public func loudest<T: Greeter>(_ xs: [T]) -> String {
    xs.map { $0.greet() }.joined()              // C17 $0.greet -> conformers
}

public func anyGreeter() -> some Greeter { English() }   // -> English

public struct Box<T> {
    public var value: T
    public init(value: T) { self.value = value }
    public func get() -> T { value }
}

public func boxed() -> Int {
    let b = Box<Int>(value: 3)                  // C17 constructor_expression -> Box
    return b.get()                              // -> Box.get
}

// C20 custom delegate protocol (AppKit-style)
public protocol EngineDelegate: AnyObject {
    func engineDidFinish(_ engine: Engine)
}

public final class Engine {
    public weak var delegate: EngineDelegate?
    public init() {}
    public func run() {
        delegate?.engineDidFinish(self)          // C20 -> any EngineDelegate conformer's engineDidFinish
    }
}
