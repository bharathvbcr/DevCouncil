package com.fx.core.util

class FlEngine {
    fun tick(): Int {
        return 1
    }
}

class FlOther {
    fun tick(): Int {
        return 2
    }
}

class FlHolderA {
    val engine: FlEngine = FlEngine()

    fun go(): Int {
        return engine.tick()
    }
}

class FlHolderB {
    val engine = FlOther()

    fun go(): Int {
        return engine.tick()
    }
}
