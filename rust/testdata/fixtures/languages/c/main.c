#include <stdio.h>
struct Service { int value; };
int run(struct Service service) { return printf("%d", service.value); }
