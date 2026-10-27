//go:build unix

package safefile

import (
	"os"
	"path/filepath"
	"syscall"
	"testing"
)

// A FIFO swapped onto the name in the window is a non-regular entry: refused
// at once, never retried, and never opened (which could block on a reader).
func TestOpenNoFollowRefusesAFIFOSwappedInDuringTheWindow(t *testing.T) {
	dir, root := rootedDir(t, "a.md")
	fifo := filepath.Join(dir, "swap.fifo")
	if err := syscall.Mkfifo(fifo, 0o600); err != nil {
		t.Skipf("mkfifo unavailable here: %v", err)
	}
	var calls int
	setAfterRootedOpen(t, func(int) {
		calls++
		if err := os.Rename(fifo, filepath.Join(dir, "a.md")); err != nil {
			t.Errorf("swap fifo in: %v", err)
		}
	})

	requireRefusal(t, root, "a.md", openFDs(t))
	if calls != 1 {
		t.Fatalf("window ran %d times; a non-regular entry must not be retried", calls)
	}
}
