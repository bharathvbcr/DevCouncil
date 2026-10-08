//go:build unix

package gussetfn

/*
#cgo noescape devcouncil_gusset_init
#cgo nocallback devcouncil_gusset_init
#cgo LDFLAGS: -L${SRCDIR}/../../../rust/gusset-engine/target/release
int devcouncil_gusset_init(void);
*/
import "C"

import "fmt"

// register installs the dc-glob engine. The symbol lives in the umbrella
// archive (rust/gusset-engine), which is the only libgusset.a this binary
// may link. Link with that directory first:
//
//	CGO_LDFLAGS="-L$(pwd)/../../rust/gusset-engine/target/release"
//
// from backend/go_orchestrator. Gusset's own -L paths are a fallback for its
// tests; if they win, this symbol is missing and the link fails closed.
//
// The umbrella answers how many of its opcodes failed to register. One that
// did not would reach the global handler, which reads its frame as a
// single-pattern match — a different question — so a failure here refuses
// every handle rather than answering with the wrong decoder.
func register() error {
	if n := C.devcouncil_gusset_init(); n != 0 {
		return fmt.Errorf("gusset: %d of the engine's opcodes failed to register", int(n))
	}
	return nil
}
