package gussetfn

/*
#cgo noescape devcouncil_gusset_init
#cgo nocallback devcouncil_gusset_init
#cgo LDFLAGS: -L${SRCDIR}/../../../rust/gusset-engine/target/release
int devcouncil_gusset_init(void);
*/
import "C"

// register installs the dc-glob engine. The symbol lives in the umbrella
// archive (rust/gusset-engine), which is the only libgusset.a this binary
// may link. Link with that directory first:
//
//	CGO_LDFLAGS="-L$(pwd)/../../rust/gusset-engine/target/release"
//
// from backend/go_orchestrator. Gusset's own -L paths are a fallback for its
// tests; if they win, this symbol is missing and the link fails closed.
func register() {
	C.devcouncil_gusset_init()
}
