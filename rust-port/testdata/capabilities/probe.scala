package probe

import com.example.Helper

class Widget extends BaseWidget with Renderable {
  var name: String = ""

  def render(): String = Helper.help(name)
}

object Main {
  def run(): Unit = {
    val w = new Widget()
    w.render()
  }
}
