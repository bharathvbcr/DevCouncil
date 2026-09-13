//go:build unix

package testsupport

import (
	"errors"
	"os"
	"path/filepath"
	"syscall"
	"testing"
)

// TestLockBuildOutputsExcludesASecondHolder asserts the one property cargoBin
// depends on, and the one a quietly broken implementation would drop without
// any other test noticing: while the lock is held another opener must not be
// able to take it, and once released it must.
//
// The contending attempt is non-blocking, so this asserts an outcome rather
// than waiting to see whether something hangs — a test that proves exclusion by
// timing out proves it only on a machine that was fast enough. flock is held
// per open file description rather than per process, so a second os.OpenFile of
// the same path contends with the first exactly as another `go test` process
// would.
func TestLockBuildOutputsExcludesASecondHolder(t *testing.T) {
	path := filepath.Join(t.TempDir(), buildLockName)

	release, err := lockBuildOutputs(path)
	if err != nil {
		t.Fatalf("lockBuildOutputs(%s): %v", path, err)
	}

	contender, err := os.OpenFile(path, os.O_CREATE|os.O_RDWR, 0o600)
	if err != nil {
		release()
		t.Fatalf("open the lock file a second time: %v", err)
	}
	defer contender.Close()

	err = syscall.Flock(int(contender.Fd()), syscall.LOCK_EX|syscall.LOCK_NB)
	switch {
	case err == nil:
		_ = syscall.Flock(int(contender.Fd()), syscall.LOCK_UN)
		release()
		t.Fatal("a second holder took the lock while it was held: the build-and-copy " +
			"in cargoBin is not serialised, and two test processes can rewrite the " +
			"artifact a third is reading")
	case !errors.Is(err, syscall.EWOULDBLOCK):
		release()
		t.Fatalf("the contending flock failed for a reason other than contention: %v", err)
	}

	release()

	if err := syscall.Flock(int(contender.Fd()), syscall.LOCK_EX|syscall.LOCK_NB); err != nil {
		t.Fatalf("the lock was still held after its release function ran: %v\n"+
			"every later cargoBin call in this process would block forever", err)
	}
	if err := syscall.Flock(int(contender.Fd()), syscall.LOCK_UN); err != nil {
		t.Fatalf("release the contending lock: %v", err)
	}
}

// TestLockBuildOutputsReportsAPathItCannotOpen covers the error path. Returning
// a release function alongside a failure would be the dangerous shape: cargoBin
// defers it and carries on copying, believing it holds a lock nobody took.
func TestLockBuildOutputsReportsAPathItCannotOpen(t *testing.T) {
	path := filepath.Join(t.TempDir(), "no-such-directory", buildLockName)

	release, err := lockBuildOutputs(path)
	if err == nil {
		release()
		t.Fatalf("lockBuildOutputs reported success for %s, which it cannot open", path)
	}
	if release != nil {
		t.Fatal("lockBuildOutputs returned both an error and a release function; " +
			"a caller that defers it copies the artifact unserialised")
	}
}
