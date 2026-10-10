package cgox

/*
#include <stdlib.h>
static int add22(int a, int b) { return a + b; }
*/
import "C"

// goSide22 is a Go helper called from the cgo function (C22).
func goSide22() int { return 22 }

// Caller22 calls C.add22 (a C function, must not bind to any Go symbol) and a Go helper.
func Caller22() int { return int(C.add22(1, 2)) + goSide22() }
