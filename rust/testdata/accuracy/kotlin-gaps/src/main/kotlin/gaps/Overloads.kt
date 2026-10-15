package gaps

class Calc {
    fun add(a: Int): Int = a

    fun add(a: String): Int = add(a.length)
}

fun useCalc(c: Calc): Int = c.add(1) + c.add("two")
