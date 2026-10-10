package com.fx.app

// K08 object singleton; K07 companion object factory
object Registry {
    private val names = mutableListOf<String>()
    fun register(name: String) { names.add(name) }
    fun unusedRegistryFn() = names.size                   // EXPECT dead (truly)
}

class Clock private constructor(val start: Long) {
    fun elapsed(now: Long): Long = now - start
    companion object {
        fun create(): Clock = Clock(0L)                    // EXPECT create -> Clock (ctor)
    }
}
