package gussetfn

import "errors"

// ErrClosed is returned after Close: the engine is shut for this process.
var ErrClosed = errors.New("gusset: engine is closed")

// ErrUnsupported is every answer on a platform gusset does not support.
// gusset is unix-only (its Rust crate refuses other targets with
// compile_error!), so a Windows build has no engine to link; it still has to
// build, because the host is built there for dc-verify's json contract.
var ErrUnsupported = errors.New("gusset: the engine is unix-only and is not built on this platform")
