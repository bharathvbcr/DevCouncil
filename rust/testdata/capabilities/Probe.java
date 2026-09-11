package probe;

import com.example.Helper;

public class Widget extends BaseWidget implements Renderable {
    private String name;

    public String render() {
        return Helper.help(this.name);
    }

    public static void main(String[] args) {
        Widget w = new Widget();
        w.render();
    }
}
