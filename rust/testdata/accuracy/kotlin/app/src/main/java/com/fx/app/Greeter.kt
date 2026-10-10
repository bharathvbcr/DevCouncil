package com.fx.app

// K10 interface default method + override; K28 same-name methods on unrelated types (false-edge bait)
interface Greeter {
    fun greet(): String = "${hello()}!"                   // EXPECT greet -> Greeter.hello
    fun hello(): String
}

class English : Greeter {
    override fun hello() = "hi"
}

class Door {
    fun hello() = "knock"                                  // not a Greeter
}

fun greetAll(g: Greeter, unknown: Any) {
    g.greet()                                               // EXPECT greetAll -> Greeter.greet
    val e = English()                                       // EXPECT greetAll -> English (ctor)
    e.hello()                                               // EXPECT greetAll -> English.hello ; NOT Door.hello
    (unknown as? Door)?.hello()                             // ambiguous receiver: Door.hello true; English.hello would be false
}
