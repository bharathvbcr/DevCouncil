import XCTest
import Testing
@testable import Core

// C22 XCTest
final class PersonTests: XCTestCase {
    func testGreeting() {
        let p = Person(name: "a", age: 2)
        XCTAssertEqual(p.greeting(), "Hi A")          // -> Person.greeting
    }
    func helperNotATest() -> Int { 1 }               // TRUE DEAD helper (not test-prefixed)
}

// C23 Swift Testing
@Suite struct VecSuite {
    @Test func adds() {
        let v = addVectors(Vec2(x: 1, y: 1), Vec2(x: 2, y: 2))   // -> addVectors, Vec2
        #expect(v.x == 3)
    }
    @Test func scales() {
        #expect(Vec2.scaled(Vec2(x: 1, y: 0), by: 2).x == 2)     // -> Vec2.scaled (inside macro)
    }
}
