// C16 enums with associated values and methods
public enum Shape {
    case circle(Double)
    case rect(Double, Double)

    public func area() -> Double {
        switch self {
        case .circle(let r): return r * r * 3.14
        case .rect(let w, let h): return w * h
        }
    }

    public static func unit() -> Shape { .circle(1) }
}

public func totalArea() -> Double {
    let s: Shape = .circle(2)
    let a = s.area()                                // C16 annotated -> Shape.area
    let b = Shape.rect(1, 2).area()                 // C16 chained case-construct -> Shape.area
    let c = Shape.unit().area()                     // C33 return-type chain -> Shape.area (known gap)
    return a + b + c
}

// C05 property wrapper definition and use
@propertyWrapper
public struct Clamped {
    private var value: Int
    public init(wrappedValue: Int) { value = min(max(wrappedValue, 0), 10) }
    public var wrappedValue: Int {
        get { value }
        set { value = min(max(newValue, 0), 10) }
    }
}

public struct Settings {
    @Clamped public var volume: Int = 5             // C05 attribute use -> Clamped
    public init() {}
}

// C06 result builder
@resultBuilder
public enum ListBuilder {
    public static func buildBlock(_ parts: String...) -> [String] { parts }   // compiler-called
}

public func makeList(@ListBuilder _ content: () -> [String]) -> [String] { content() }

public func builtList() -> [String] {
    makeList { "a"; "b" }                          // -> makeList ; buildBlock called by compiler
}
