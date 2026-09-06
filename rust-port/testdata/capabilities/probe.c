#include <stdio.h>
#include "helper.h"

struct Widget {
    const char *name;
};

const char *widget_render(struct Widget *w) {
    return helper_help(w->name);
}

int main(void) {
    struct Widget w;
    widget_render(&w);
    return 0;
}
