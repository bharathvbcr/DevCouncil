package safefile

import (
	"io"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// These tests place a publisher's rename in the window between a rooted open
// and the Lstat that samples its name, using the afterRootedOpen seam. They
// share a package-level hook, so none may run in parallel.

const identityRefusal = "opened entry changed identity or is a symbolic link"

// setAfterRootedOpen installs fn as the window hook for the test's lifetime.
func setAfterRootedOpen(t *testing.T, fn func(attempt int)) {
	t.Helper()
	prev := afterRootedOpen
	afterRootedOpen = fn
	t.Cleanup(func() { afterRootedOpen = prev })
}

// rootedDir returns a directory that already holds name with content "v0",
// and a root held over it.
func rootedDir(t *testing.T, name string) (string, *os.Root) {
	t.Helper()
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, name), []byte("v0"), 0o600); err != nil {
		t.Fatal(err)
	}
	root, err := os.OpenRoot(dir)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = root.Close() })
	return dir, root
}

// renameOnto publishes a regular file with content onto name, as an atomic
// publisher does. It reports failure with Errorf so it is safe off the test
// goroutine.
func renameOnto(t *testing.T, dir, name, content string) {
	t.Helper()
	tmp := filepath.Join(dir, "publish.tmp")
	if err := os.WriteFile(tmp, []byte(content), 0o600); err != nil {
		t.Errorf("write publisher temp: %v", err)
		return
	}
	if err := os.Rename(tmp, filepath.Join(dir, name)); err != nil {
		t.Errorf("rename onto %s: %v", name, err)
	}
}

// openFDs lists the process's open descriptors, or skips where /dev/fd is not
// readable. The listing's own directory descriptor is present in every sample.
func openFDs(t *testing.T) int {
	t.Helper()
	entries, err := os.ReadDir("/dev/fd")
	if err != nil {
		t.Skipf("descriptor listing unavailable on this platform: %v", err)
	}
	return len(entries)
}

// openBounded runs OpenNoFollow and fails the test if it does not return, so
// an unbounded retry shows up as a failure rather than a hung run.
func openBounded(t *testing.T, root *os.Root, name string) (*os.File, error) {
	t.Helper()
	type result struct {
		f   *os.File
		err error
	}
	done := make(chan result, 1)
	go func() {
		f, err := OpenNoFollow(root, name, os.O_RDONLY, 0)
		done <- result{f, err}
	}()
	select {
	case r := <-done:
		return r.f, r.err
	case <-time.After(10 * time.Second):
		t.Fatal("OpenNoFollow did not return: the retry is unbounded")
		return nil, nil
	}
}

// requireRefusal asserts that OpenNoFollow returned no handle and the identity
// error, and that no descriptor outlived the call.
func requireRefusal(t *testing.T, root *os.Root, name string, fdsBefore int) {
	t.Helper()
	f, err := openBounded(t, root, name)
	if f != nil {
		_ = f.Close()
		t.Fatalf("a handle was returned for %s", name)
	}
	if err == nil || !strings.Contains(err.Error(), identityRefusal) {
		t.Fatalf("want the identity refusal, got %v", err)
	}
	if after := openFDs(t); after != fdsBefore {
		t.Fatalf("descriptor leak: %d open before, %d after", fdsBefore, after)
	}
}

// A regular file renamed onto the name in the window is the file the caller
// named: the open is retried and the returned handle reads the new content.
// Without the retry the first handle (the old inode) disagrees with the name
// and the call fails.
func TestOpenNoFollowRetriesWhenARegularFileIsRenamedOntoTheName(t *testing.T) {
	dir, root := rootedDir(t, "a.md")
	var attempts []int
	setAfterRootedOpen(t, func(attempt int) {
		attempts = append(attempts, attempt)
		if attempt == 0 {
			renameOnto(t, dir, "a.md", "v1")
		}
	})
	fdsBefore := openFDs(t)

	f, err := OpenNoFollow(root, "a.md", os.O_RDONLY, 0)
	if err != nil {
		t.Fatalf("replaced regular file refused: %v", err)
	}
	got, readErr := io.ReadAll(f)
	if closeErr := f.Close(); readErr != nil || closeErr != nil {
		t.Fatalf("read %v close %v", readErr, closeErr)
	}
	if string(got) != "v1" {
		t.Fatalf("handle reads %q, want the name's current file %q", got, "v1")
	}
	if len(attempts) != 2 || attempts[0] != 0 || attempts[1] != 1 {
		t.Fatalf("window attempts %v, want [0 1]", attempts)
	}
	if after := openFDs(t); after != fdsBefore {
		t.Fatalf("descriptor leak: %d open before, %d after", fdsBefore, after)
	}
}

// A symlink swapped onto the name in the window is refused at once, never
// retried, even though its target is a regular file.
func TestOpenNoFollowRefusesASymlinkSwappedInDuringTheWindow(t *testing.T) {
	dir, root := rootedDir(t, "a.md")
	secret := filepath.Join(t.TempDir(), "secret")
	if err := os.WriteFile(secret, []byte("secret"), 0o600); err != nil {
		t.Fatal(err)
	}
	link := filepath.Join(dir, "swap.lnk")
	if err := os.Symlink(secret, link); err != nil {
		t.Skipf("symlinks unavailable: %v", err)
	}
	var calls int
	setAfterRootedOpen(t, func(int) {
		calls++
		if err := os.Rename(link, filepath.Join(dir, "a.md")); err != nil {
			t.Errorf("swap symlink in: %v", err)
		}
	})

	requireRefusal(t, root, "a.md", openFDs(t))
	if calls != 1 {
		t.Fatalf("window ran %d times; a symlink must not be retried", calls)
	}
}

// A rename on every attempt exhausts the bounded retry and reports the
// identity error rather than looping.
func TestOpenNoFollowGivesUpAfterTheAttemptCap(t *testing.T) {
	dir, root := rootedDir(t, "a.md")
	var calls int
	setAfterRootedOpen(t, func(int) {
		calls++
		renameOnto(t, dir, "a.md", "next")
	})

	requireRefusal(t, root, "a.md", openFDs(t))
	if calls != rootedIdentityAttempts {
		t.Fatalf("window ran %d times, want the cap %d", calls, rootedIdentityAttempts)
	}
}
