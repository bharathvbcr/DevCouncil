package com.fx.app

import org.junit.Assert.assertEquals
import org.junit.Test

// K20 JUnit test
class ClockTest {
    @Test
    fun elapsedCountsFromStart() {
        val clock = Clock.create()                         // EXPECT test -> Clock.create
        assertEquals(5L, clock.elapsed(5L))                 // EXPECT test -> Clock.elapsed
    }
}
