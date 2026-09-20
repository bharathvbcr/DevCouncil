//go:build unix

// devcouncil: allow-unwired
//
// Test-only by design, and shared with Manvi; see testsupport.go. Build-tagged
// as well, so only one of this file and buildlock_other.go is ever compiled —
// a second reason a static walk finds no caller for whichever half is not the
// current platform's.
package testsupport

import (
	"os"
	"syscall"
)

// lockBuildOutputs takes an exclusive, cross-process lock on the Rust build
// outputs and returns the release function.
//
// `go test ./...` runs each package in its own process, so the per-process
// sync.Once in cargoBin cannot serialise anything: several test binaries reach
// cargo concurrently. Cargo's own workspace lock serialises the compile, but it
// is released before the artifact has been copied out of the way, which leaves
// the window this lock closes.
func lockBuildOutputs(path string) (func(), error) {
	f, err := os.OpenFile(path, os.O_CREATE|os.O_RDWR, 0o600)
	if err != nil {
		return nil, err
	}
	if err := syscall.Flock(int(f.Fd()), syscall.LOCK_EX); err != nil {
		f.Close()
		return nil, err
	}
	return func() {
		syscall.Flock(int(f.Fd()), syscall.LOCK_UN)
		f.Close()
	}, nil
}
