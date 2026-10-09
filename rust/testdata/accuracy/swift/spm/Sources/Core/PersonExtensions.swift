// C08 extension in another file adds methods to Person
extension Person {
    public func fullName() -> String { name.uppercased() }
    public func greeting() -> String { "Hi " + fullName() }    // -> Person.fullName (implicit self)
}

// C18 operator overload
public struct Vec2: Equatable {
    public var x: Double, y: Double
    public init(x: Double, y: Double) { self.x = x; self.y = y }
    public static func + (l: Vec2, r: Vec2) -> Vec2 { Vec2(x: l.x + r.x, y: l.y + r.y) }
    public static func scaled(_ v: Vec2, by k: Double) -> Vec2 { Vec2(x: v.x * k, y: v.y * k) }
}

public func addVectors(_ a: Vec2, _ b: Vec2) -> Vec2 {
    a + b                                   // C18 -> Vec2.+ (operator)
}
