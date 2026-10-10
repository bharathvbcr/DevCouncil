import Foundation

// C09 init / convenience init; C10 static + class methods; C11 computed property;
// C32 implicit-self member call; C34 typed stored property receiver.
public struct Person: Equatable {
    public var name: String
    public var age: Int

    public init(name: String, age: Int) {
        self.name = name
        self.age = age
        validateAge()                      // C09: call from init body -> Person.validateAge
    }

    public init(name: String) {
        self.init(name: name, age: 0)      // C09: self.init -> no edge expected (same type)
    }

    // C11 computed property: calls inside attribute to the owner
    public var display: String { formatDisplay() }   // -> Person.formatDisplay

    func validateAge() { precondition(age >= 0) }
    func formatDisplay() -> String { "\(name) (\(age))" }

    public static func make(named n: String) -> Person {   // C10
        Person(name: n, age: 1)                              // -> Person (ctor)
    }

    public static func defaultPerson() -> Person {
        Self.make(named: "anon")                             // C10 Self. -> Person.make
    }
}

public final class Registry {
    public let store: PersonStore           // C34 typed stored property
    public init(store: PersonStore) { self.store = store }

    public class func shared() -> Registry {     // C10 class func
        Registry(store: PersonStore())           // -> Registry, PersonStore ctors
    }

    public func register(_ p: Person) {
        store.save(p)                            // C34 -> PersonStore.save
        self.store.audit()                       // C34 self.-prefixed -> PersonStore.audit
        notifyAll()                              // C32 implicit self -> Registry.notifyAll
    }

    func notifyAll() {}
}

public final class PersonStore {
    public init() {}
    public func save(_ p: Person) {}
    public func audit() {}
    public func unusedStoreHelper() {}           // TRUE DEAD (internal-visible though)
}
