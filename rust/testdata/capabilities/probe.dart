import 'package:example/helper.dart';

class Widget extends BaseWidget implements Renderable {
  String name = '';

  String render() {
    return helper(name);
  }
}

void mainEntry() {
  final w = Widget();
  w.render();
}
