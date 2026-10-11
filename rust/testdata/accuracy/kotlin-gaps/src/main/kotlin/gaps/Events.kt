package gaps

import gaps.util.format as fmt

class Widget {
    fun onStart(n: Int) {}
}

class Prefs {
    fun putBoolean(key: String, value: Boolean) {}
}

fun runWith(onStart: (Int) -> Unit) {
    onStart(1)
}

fun save(editor: android.content.SharedPreferences.Editor) {
    editor.apply { putBoolean("k", true) }
}

class Vm {
    fun onEvent(e: Int) {}
}

fun register(f: (Int) -> Unit) {}

fun wire(vm: Vm) {
    register(vm::onEvent)
}

class Op {
    operator fun invoke(x: Int): Int = x
}

fun callOp(): Int {
    val op = Op()
    return op(2)
}

fun useAlias(): String = fmt(3)

open class Base {
    open fun go() {}
}

class Child : Base() {
    override fun go() {
        super.go()
    }
}

class Factory {
    companion object {
        fun build(): Factory = Factory()
    }
}

fun useFactory(): Factory = Factory.build()

sealed class Res {
    class Ok(val v: Int) : Res()
}

fun mk(): Res = Res.Ok(1)
