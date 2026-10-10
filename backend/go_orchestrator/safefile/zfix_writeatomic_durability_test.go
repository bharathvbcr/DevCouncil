package safefile

import (
	"errors"
	"os"
	"path/filepath"
	"testing"
)

func failStep[T any](t *testing.T, seam *T, replacement T) {
	t.Helper()
	old := *seam
	*seam = replacement
	t.Cleanup(func() { *seam = old })
}

func TestWriteAtomicReturnsAFileSyncFailureAndKeepsTheOriginal(t *testing.T) {
	path := filepath.Join(t.TempDir(), "f")
	if err := os.WriteFile(path, []byte("original"), 0o644); err != nil {
		t.Fatal(err)
	}
	failStep(t, &syncFile, func(*os.File) error { return errors.New("injected fsync failure") })

	if err := WriteAtomic(path, []byte("replacement"), 0o644); err == nil {
		t.Fatal("a failed fsync was reported as a successful write")
	}
	if got, _ := os.ReadFile(path); string(got) != "original" {
		t.Fatalf("the original was lost: %q", got)
	}
	entries, _ := os.ReadDir(filepath.Dir(path))
	if len(entries) != 1 {
		t.Fatalf("a failed write left a temp file behind: %v", entries)
	}
}

func TestWriteAtomicReturnsADirectorySyncFailure(t *testing.T) {
	path := filepath.Join(t.TempDir(), "f")
	failStep(t, &syncDir, func(string) error { return errors.New("injected directory sync failure") })
	if err := WriteAtomic(path, []byte("x"), 0o644); err == nil {
		t.Fatal("a failed directory sync was reported as a successful write")
	}
}

func TestWriteAtomicSyncsTheFileThenTheDirectory(t *testing.T) {
	path := filepath.Join(t.TempDir(), "f")
	var order []string
	failStep(t, &syncFile, func(f *os.File) error { order = append(order, "file"); return f.Sync() })
	failStep(t, &syncDir, func(d string) error { order = append(order, "dir"); return syncDirectory(d) })
	if err := WriteAtomic(path, []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	if len(order) != 2 || order[0] != "file" || order[1] != "dir" {
		t.Fatalf("durability steps = %v, want file then dir", order)
	}
}
