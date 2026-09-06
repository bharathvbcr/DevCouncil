package probe

import com.example.helper

open class BaseWidget

class Widget : BaseWidget(), Renderable {
    var name: String = ""

    override fun render(): String {
        return helper(name)
    }
}

fun mainEntry() {
    val w = Widget()
    w.render()
}
