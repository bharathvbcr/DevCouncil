package secret

// #include <stdint.h>
import "C"

// W3: exported to C by cgo. The C side is outside the corpus; it stays exempt.
//
//export SecretCallback
func SecretCallback() C.int { return 0 }
