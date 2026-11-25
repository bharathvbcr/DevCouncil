package components

import (
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func asideFixture(t *testing.T) (dir, dest string) {
	t.Helper()
	dir = t.TempDir()
	dest = filepath.Join(dir, "tool.exe")
	if err := os.WriteFile(dest, []byte("old"), 0o755); err != nil {
		t.Fatal(err)
	}
	return dir, dest
}

func asideEntries(t *testing.T, dir string) []string {
	t.Helper()
	entries, err := os.ReadDir(dir)
	if err != nil {
		t.Fatal(err)
	}
	var names []string
	for _, e := range entries {
		if strings.Contains(e.Name(), asideMarker) {
			names = append(names, e.Name())
		}
	}
	return names
}

// TestReplaceAsideLandsTheNewCopyAndDropsTheOld: the second write sees dest
// free, the new bytes take its name, and an old copy nothing holds is gone.
func TestReplaceAsideLandsTheNewCopyAndDropsTheOld(t *testing.T) {
	dir, dest := asideFixture(t)
	refused := errors.New("in use")
	write := func() error {
		if _, err := os.Stat(dest); !os.IsNotExist(err) {
			t.Fatalf("second write ran with dest still in place: %v", err)
		}
		return os.WriteFile(dest, []byte("new"), 0o755)
	}
	if err := replaceAside(dest, refused, write); err != nil {
		t.Fatal(err)
	}
	if got, _ := os.ReadFile(dest); string(got) != "new" {
		t.Fatalf("dest = %q", got)
	}
	if left := asideEntries(t, dir); len(left) != 0 {
		t.Fatalf("aside copies left: %q", left)
	}
}

// TestReplaceAsideRestoresTheOldCopyWhenTheRetryFails: a failed retry must not
// leave the install with no binary at all.
func TestReplaceAsideRestoresTheOldCopyWhenTheRetryFails(t *testing.T) {
	dir, dest := asideFixture(t)
	retry := errors.New("disk full")
	err := replaceAside(dest, errors.New("in use"), func() error { return retry })
	if !errors.Is(err, retry) {
		t.Fatalf("err = %v, want the retry's error", err)
	}
	if got, _ := os.ReadFile(dest); string(got) != "old" {
		t.Fatalf("dest = %q, want the old copy restored", got)
	}
	if left := asideEntries(t, dir); len(left) != 0 {
		t.Fatalf("aside copies left: %q", left)
	}
}

// TestReplaceAsideReportsTheRefusalWhenThereIsNothingToMove: a refusal with no
// existing dest is not an in-use binary, so it comes back as it was.
func TestReplaceAsideReportsTheRefusalWhenThereIsNothingToMove(t *testing.T) {
	dest := filepath.Join(t.TempDir(), "tool.exe")
	refused := errors.New("refused")
	called := false
	err := replaceAside(dest, refused, func() error { called = true; return nil })
	if !errors.Is(err, refused) || called {
		t.Fatalf("err = %v, retried = %v; want the refusal and no retry", err, called)
	}
}

// TestReplaceAsideNeverMovesADirectory: a directory named like the binary is
// not an in-use program. Windows CI caught the first version moving it aside
// and installing over its name (TestAPartialInstallRecordsWhatItPutOnDisk).
func TestReplaceAsideNeverMovesADirectory(t *testing.T) {
	dir := t.TempDir()
	dest := filepath.Join(dir, "tool.exe")
	if err := os.MkdirAll(dest, 0o755); err != nil {
		t.Fatal(err)
	}
	keep := filepath.Join(dest, "keep")
	if err := os.WriteFile(keep, []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	refused := errors.New("is a directory")
	called := false
	err := replaceAside(dest, refused, func() error { called = true; return nil })
	if !errors.Is(err, refused) || called {
		t.Fatalf("err = %v, retried = %v; want the refusal and no retry", err, called)
	}
	if _, err := os.Stat(keep); err != nil {
		t.Fatalf("the directory's contents moved: %v", err)
	}
	if left := asideEntries(t, dir); len(left) != 0 {
		t.Fatalf("aside copies left: %q", left)
	}
}

// TestSweepAsideRemovesOnlyThisBinarysAsideCopies: siblings that merely share a
// prefix, and other binaries' aside copies, are not this binary's to remove.
func TestSweepAsideRemovesOnlyThisBinarysAsideCopies(t *testing.T) {
	dir, dest := asideFixture(t)
	mine := filepath.Join(dir, "tool.exe"+asideMarker+"1")
	keep := []string{
		filepath.Join(dir, "other.exe"+asideMarker+"1"),
		filepath.Join(dir, "tool.exe.bak"),
	}
	for _, p := range append([]string{mine}, keep...) {
		if err := os.WriteFile(p, nil, 0o644); err != nil {
			t.Fatal(err)
		}
	}
	sweepAside(dest)
	if _, err := os.Stat(mine); !os.IsNotExist(err) {
		t.Fatalf("this binary's aside copy survived: %v", err)
	}
	for _, p := range append([]string{dest}, keep...) {
		if _, err := os.Stat(p); err != nil {
			t.Fatalf("%s was removed: %v", filepath.Base(p), err)
		}
	}
}
