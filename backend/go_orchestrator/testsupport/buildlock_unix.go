//go:build unix

// devcouncil: allow-unwired
//
// Test-only by design, and shared with Manvi; see testsupport.go. Build-tagged
// as well, so only one of this file and buildlock_other.go is ever compiled —
// a second reason a static walk finds no caller for whichever half is not the
// current platform's.
package testsupport

import (
	"context"
	"errors"
	"fmt"
	"os"
	"syscall"
	"time"
)

// lockPollInterval is how often a contended lock is retried.
const lockPollInterval = 100 * time.Millisecond

// lockBuildOutputs takes an exclusive, cross-process lock on the Rust build
// outputs and returns the release function. It gives up when ctx ends, with an
// error wrapping errOverran.
//
// `go test ./...` runs each package in its own process, so the per-process
// sync.Once in cargoBin cannot serialise anything: several test binaries reach
// cargo concurrently. Cargo's own workspace lock serialises the compile, but it
// is released before the artifact has been copied out of the way, which leaves
// the window this lock closes.
//
// The wait polls a non-blocking flock rather than parking in a blocking one.
// A blocking flock cannot be cancelled, and one abandoned on a goroutine would
// take the lock after its caller had given up and hold it, for nobody, until
// the process exited. The holder is another test process building in the same
// workspace, and it can hold the lock for as long as its own build takes; an
// unbounded wait here outlasted `go test`'s alarm, which then reported the
// wait as a goroutine dump instead of naming it.
func lockBuildOutputs(ctx context.Context, path string) (func(), error) {
	f, err := os.OpenFile(path, os.O_CREATE|os.O_RDWR, 0o600)
	if err != nil {
		return nil, err
	}
	start := time.Now()
	ticker := time.NewTicker(lockPollInterval)
	defer ticker.Stop()
	for {
		err := syscall.Flock(int(f.Fd()), syscall.LOCK_EX|syscall.LOCK_NB)
		if err == nil {
			return func() {
				syscall.Flock(int(f.Fd()), syscall.LOCK_UN)
				f.Close()
			}, nil
		}
		if !errors.Is(err, syscall.EWOULDBLOCK) && !errors.Is(err, syscall.EINTR) {
			f.Close()
			return nil, err
		}
		select {
		case <-ctx.Done():
			f.Close()
			return nil, fmt.Errorf("%w: waited %s for the build lock %s, held by another test process "+
				"building in this workspace", errOverran, time.Since(start).Round(time.Second), path)
		case <-ticker.C:
		}
	}
}
