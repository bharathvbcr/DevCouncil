package safefile

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sync"
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

func TestWriteAtomicFailedRenameLeavesThePreviousFileAndNoTemp(t *testing.T) {
	path := filepath.Join(t.TempDir(), "config.yaml")
	if err := os.WriteFile(path, []byte("gates:\n  mode: enforce\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	var calls int
	failStep(t, &renameFile, func(string, string) error { calls++; return errors.New("injected rename failure") })

	if err := WriteAtomic(path, []byte("replacement"), 0o644); err == nil {
		t.Fatal("a failed rename was reported as a successful write")
	}
	if calls != 1 {
		t.Fatalf("rename attempted %d times; a retry must never be preceded by deleting the target", calls)
	}
	if got, _ := os.ReadFile(path); string(got) != "gates:\n  mode: enforce\n" {
		t.Fatalf("the live file was lost or changed: %q", got)
	}
	if entries, _ := os.ReadDir(filepath.Dir(path)); len(entries) != 1 {
		t.Fatalf("a failed write left a temp file behind: %v", entries)
	}
}

func TestWriteAtomicCrashAtEachStepNeverLeavesAPartialTarget(t *testing.T) {
	steps := map[string]func(){
		"file-sync": func() { failStep(t, &syncFile, func(*os.File) error { return errors.New("crash") }) },
		"rename":    func() { failStep(t, &renameFile, func(string, string) error { return errors.New("crash") }) },
	}
	for name, inject := range steps {
		t.Run(name, func(t *testing.T) {
			path := filepath.Join(t.TempDir(), "f")
			if err := os.WriteFile(path, []byte("old"), 0o644); err != nil {
				t.Fatal(err)
			}
			inject()
			if err := WriteAtomic(path, []byte("new"), 0o644); err == nil {
				t.Fatal("injected failure was swallowed")
			}
			if got, _ := os.ReadFile(path); string(got) != "old" {
				t.Fatalf("target = %q, want the untouched original", got)
			}
		})
	}
}

func TestWriteAtomicConcurrentWritersAlwaysLeaveAWholeFile(t *testing.T) {
	path := filepath.Join(t.TempDir(), "f")
	if err := os.WriteFile(path, []byte("seed-0000"), 0o644); err != nil {
		t.Fatal(err)
	}
	var wg sync.WaitGroup
	stop := make(chan struct{})
	bad := make(chan string, 1)
	go func() {
		for {
			select {
			case <-stop:
				return
			default:
			}
			got, err := os.ReadFile(path)
			if err != nil || len(got) != 9 {
				select {
				case bad <- fmt.Sprintf("read %q err=%v", got, err):
				default:
				}
				return
			}
		}
	}()
	for w := 0; w < 8; w++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for i := 0; i < 50; i++ {
				if err := WriteAtomic(path, []byte(fmt.Sprintf("w%d-%06d", w, i)), 0o644); err != nil {
					t.Error(err)
					return
				}
			}
		}()
	}
	wg.Wait()
	close(stop)
	select {
	case msg := <-bad:
		t.Fatalf("a reader saw a missing or partial file: %s", msg)
	default:
	}
}
