package components

import (
	"bytes"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"testing"
)

// Two installers racing on one destination must each leave a whole binary:
// whichever rename lands last wins, and no torn mix of the two, and no
// temporary, is ever left behind.
func TestConcurrentInstallsNeverTearTheBinary(t *testing.T) {
	dir := t.TempDir()
	const size = 4 << 20
	a := bytes.Repeat([]byte{'a'}, size)
	b := bytes.Repeat([]byte{'b'}, size)
	srcA, srcB := filepath.Join(dir, "a"), filepath.Join(dir, "b")
	if err := os.WriteFile(srcA, a, 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(srcB, b, 0o644); err != nil {
		t.Fatal(err)
	}
	dest := filepath.Join(dir, "bin", "tool")
	for round := 0; round < 40; round++ {
		var wg sync.WaitGroup
		errs := make(chan error, 8)
		for i := 0; i < 8; i++ {
			src := srcA
			if i%2 == 1 {
				src = srcB
			}
			wg.Add(1)
			go func() {
				defer wg.Done()
				errs <- installAtomic(src, dest)
			}()
		}
		wg.Wait()
		close(errs)
		for err := range errs {
			if err != nil {
				t.Fatalf("round %d: %v", round, err)
			}
		}
		got, err := os.ReadFile(dest)
		if err != nil {
			t.Fatal(err)
		}
		if !bytes.Equal(got, a) && !bytes.Equal(got, b) {
			t.Fatalf("round %d: installed binary is a torn mix of two installs (%d bytes)", round, len(got))
		}
	}
	entries, err := os.ReadDir(filepath.Dir(dest))
	if err != nil {
		t.Fatal(err)
	}
	if len(entries) != 1 {
		var names []string
		for _, e := range entries {
			names = append(names, e.Name())
		}
		t.Fatalf("temporaries left beside the binary: %v", names)
	}
}

// A prefix reached through a symlink is the same directory PATH names by its
// real path; that is not a shadowing install.
func TestResolutionNotesSeeThroughASymlinkedPrefix(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("symlinks")
	}
	real := t.TempDir()
	link := filepath.Join(t.TempDir(), "prefix")
	if err := os.Symlink(real, link); err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(filepath.Join(real, "bin"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(real, "bin", "devmap"), []byte("x"), 0o755); err != nil {
		t.Fatal(err)
	}
	lookPath := func(string) (string, error) { return filepath.Join(real, "bin", "devmap"), nil }
	if notes := resolutionNotes(filepath.Join(link, "bin"), []string{"devmap"}, lookPath); len(notes) != 0 {
		t.Fatalf("the installed copy was reported as shadowed: %v", notes)
	}
	other := filepath.Join(t.TempDir(), "devmap")
	if err := os.WriteFile(other, []byte("y"), 0o755); err != nil {
		t.Fatal(err)
	}
	shadow := func(string) (string, error) { return other, nil }
	if notes := resolutionNotes(filepath.Join(link, "bin"), []string{"devmap"}, shadow); len(notes) != 1 || !strings.Contains(notes[0], "shadows") {
		t.Fatalf("a different binary earlier on PATH was not reported: %v", notes)
	}
}
