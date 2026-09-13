package proc

import (
	"errors"
	"os"
	"path/filepath"
	"testing"
)

func writeExecutable(t *testing.T, path string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte("#!/bin/sh\nexit 0\n"), 0o755); err != nil {
		t.Fatal(err)
	}
}

// A build output inside the repository is repository content. Finding it on
// PATH does not make it a trusted installation.
func TestLookPathOutsideRefusesRepositoryLocalCandidate(t *testing.T) {
	base := t.TempDir()
	root := filepath.Join(base, "repo")
	writeExecutable(t, filepath.Join(root, "target", "release", "tool-under-test"))
	t.Setenv("PATH", filepath.Join(root, "target", "release"))

	got, err := LookPathOutside("tool-under-test", root)
	if !errors.Is(err, ErrRepositoryLocal) {
		t.Fatalf("resolved a repository-local candidate: %q (err %v)", got, err)
	}
}

// The spelling of the candidate is not the question; its identity is. A PATH
// entry that is a symlink into the repository is still repository content.
func TestLookPathOutsideRefusesSymlinkIntoRepository(t *testing.T) {
	base := t.TempDir()
	root := filepath.Join(base, "repo")
	bin := filepath.Join(base, "bin")
	writeExecutable(t, filepath.Join(root, "target", "release", "tool-under-test"))
	if err := os.MkdirAll(bin, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(filepath.Join(root, "target", "release", "tool-under-test"),
		filepath.Join(bin, "tool-under-test")); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", bin)

	got, err := LookPathOutside("tool-under-test", root)
	if !errors.Is(err, ErrRepositoryLocal) {
		t.Fatalf("followed a symlink into the repository: %q (err %v)", got, err)
	}
}

// An alternate spelling of the same root is the same root.
func TestLookPathOutsideRefusesUncanonicalRootSpelling(t *testing.T) {
	base := t.TempDir()
	root := filepath.Join(base, "repo")
	writeExecutable(t, filepath.Join(root, "bin", "tool-under-test"))
	t.Setenv("PATH", filepath.Join(root, "bin"))

	spelled := filepath.Join(root, "sub", "..")
	if err := os.MkdirAll(filepath.Join(root, "sub"), 0o755); err != nil {
		t.Fatal(err)
	}
	got, err := LookPathOutside("tool-under-test", spelled)
	if !errors.Is(err, ErrRepositoryLocal) {
		t.Fatalf("an alternate root spelling let repository content through: %q (err %v)", got, err)
	}
}

// A genuine installation outside the repository is still selected.
func TestLookPathOutsideAcceptsInstallationOutsideRepository(t *testing.T) {
	base := t.TempDir()
	root := filepath.Join(base, "repo")
	if err := os.MkdirAll(root, 0o755); err != nil {
		t.Fatal(err)
	}
	installed := filepath.Join(base, "usr", "bin", "tool-under-test")
	writeExecutable(t, installed)
	t.Setenv("PATH", filepath.Dir(installed))

	got, err := LookPathOutside("tool-under-test", root)
	if err != nil {
		t.Fatalf("refused a legitimate installation: %v", err)
	}
	wantReal, err := filepath.EvalSymlinks(installed)
	if err != nil {
		t.Fatal(err)
	}
	if got != wantReal {
		t.Fatalf("resolved %q, want %q", got, wantReal)
	}
}

// A directory named like the program is not a program.
func TestLookPathOutsideRefusesNonRegularCandidate(t *testing.T) {
	base := t.TempDir()
	root := filepath.Join(base, "repo")
	if err := os.MkdirAll(root, 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", filepath.Join(base, "empty"))
	if _, err := LookPathOutside("tool-under-test", root); err == nil {
		t.Fatal("resolved a program that is not on PATH")
	}
}
