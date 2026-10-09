package proc

import (
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
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

// componentFixture returns a repository root and an empty home, with PATH
// holding nothing.
func componentFixture(t *testing.T) (root, home string) {
	t.Helper()
	base := t.TempDir()
	root = filepath.Join(base, "repo")
	home = filepath.Join(base, "home")
	for _, dir := range []string{root, home, filepath.Join(base, "empty")} {
		if err := os.MkdirAll(dir, 0o755); err != nil {
			t.Fatal(err)
		}
	}
	t.Setenv("PATH", filepath.Join(base, "empty"))
	t.Setenv("HOME", home)
	return root, home
}

// ~/.local/bin is searched when PATH has no candidate at all.
func TestResolveComponentFallsBackToLocalBin(t *testing.T) {
	root, home := componentFixture(t)
	installed := filepath.Join(home, ".local", "bin", "tool-under-test")
	writeExecutable(t, installed)

	got, err := ResolveComponent("tool-under-test", root, Override{})
	if err != nil {
		t.Fatalf("did not search ~/.local/bin: %v", err)
	}
	want, err := filepath.EvalSymlinks(installed)
	if err != nil {
		t.Fatal(err)
	}
	if got != want {
		t.Fatalf("resolved %q, want %q", got, want)
	}
}

// A refused PATH candidate stays refused. Falling through to ~/.local/bin
// would hide that something in the repository is on PATH.
func TestResolveComponentDoesNotFallPastARefusal(t *testing.T) {
	root, home := componentFixture(t)
	writeExecutable(t, filepath.Join(home, ".local", "bin", "tool-under-test"))
	writeExecutable(t, filepath.Join(root, "bin", "tool-under-test"))
	t.Setenv("PATH", filepath.Join(root, "bin"))

	got, err := ResolveComponent("tool-under-test", root, Override{})
	if !errors.Is(err, ErrRepositoryLocal) {
		t.Fatalf("a refusal fell through to another rung: %q (err %v)", got, err)
	}
}

// The fallback rung is held to the same check: a home directory that is the
// repository does not make its ~/.local/bin a trusted installation.
func TestResolveComponentRefusesLocalBinInsideRoot(t *testing.T) {
	_, home := componentFixture(t)
	writeExecutable(t, filepath.Join(home, ".local", "bin", "tool-under-test"))

	got, err := ResolveComponent("tool-under-test", home, Override{})
	if !errors.Is(err, ErrRepositoryLocal) {
		t.Fatalf("resolved a repository-local ~/.local/bin candidate: %q (err %v)", got, err)
	}
}

// A file that is not executable is not a program, on either rung.
func TestResolveComponentRefusesNonExecutableLocalBin(t *testing.T) {
	root, home := componentFixture(t)
	path := filepath.Join(home, ".local", "bin", "tool-under-test")
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte("not a program"), 0o644); err != nil {
		t.Fatal(err)
	}

	if got, err := ResolveComponent("tool-under-test", root, Override{}); err == nil {
		t.Fatalf("resolved a file without the execute bit: %q", got)
	}
}

// Found nowhere is distinguishable from refused, so a caller can say
// "install it" rather than "something in the repository is on PATH".
func TestResolveComponentNotFoundIsErrNotFound(t *testing.T) {
	root, _ := componentFixture(t)

	_, err := ResolveComponent("tool-under-test", root, Override{})
	if !errors.Is(err, exec.ErrNotFound) {
		t.Fatalf("err = %v, want exec.ErrNotFound", err)
	}
}

// An override is used as named, including a path inside the repository:
// setting it is an operator's act, not something the repository arranges.
func TestResolveComponentOverrideIsUsed(t *testing.T) {
	root, _ := componentFixture(t)
	inside := filepath.Join(root, "bin", "tool-under-test")
	writeExecutable(t, inside)

	got, err := ResolveComponent("tool-under-test", root, Override{Path: inside, Source: "TOOL_BIN"})
	if err != nil || got != inside {
		t.Fatalf("the override was not honoured: %q (err %v)", got, err)
	}
}

// An override that names nothing, or a directory, is refused and never
// replaced by a search, and the refusal names where it came from.
func TestResolveComponentOverrideIsRefusedNotReplaced(t *testing.T) {
	root, home := componentFixture(t)
	writeExecutable(t, filepath.Join(home, ".local", "bin", "tool-under-test"))
	for _, path := range []string{filepath.Join(root, "missing"), root} {
		got, err := ResolveComponent("tool-under-test", root, Override{Path: path, Source: "TOOL_BIN"})
		if err == nil {
			t.Fatalf("override %s was replaced by %q", path, got)
		}
		if !strings.Contains(err.Error(), "TOOL_BIN") {
			t.Fatalf("refusal does not name its source: %v", err)
		}
	}
}

// Whitespace is not a path.
func TestEnvOverrideTreatsBlankAsUnset(t *testing.T) {
	t.Setenv("TOOL_BIN", "  ")
	if o := EnvOverride("TOOL_BIN"); o.Path != "" || o.Source != "TOOL_BIN" {
		t.Fatalf("EnvOverride = %+v", o)
	}
}

// A relative PATH entry names a directory relative to wherever the process
// happens to run, which is the repository. It is refused, and the refusal
// does not fall through to ~/.local/bin.
func TestResolveComponentRefusesRelativePATHEntry(t *testing.T) {
	root, home := componentFixture(t)
	writeExecutable(t, filepath.Join(root, "bin", "tool-under-test"))
	writeExecutable(t, filepath.Join(home, ".local", "bin", "tool-under-test"))
	t.Setenv("PATH", "bin")
	t.Chdir(root)

	got, err := ResolveComponent("tool-under-test", root, Override{})
	if err == nil {
		t.Fatalf("resolved %q through a relative PATH entry", got)
	}
	if errors.Is(err, exec.ErrNotFound) {
		t.Fatalf("a relative-entry refusal read as not found: %v", err)
	}
}

// What was checked is what runs. A relative override is resolved against
// the working directory once, here, and handed on absolute: a client that
// runs in the repository root would otherwise execute a different file from
// the one that was checked.
func TestResolveComponentOverrideIsMadeAbsolute(t *testing.T) {
	root, _ := componentFixture(t)
	cwd := t.TempDir()
	checked := filepath.Join(cwd, "bin", "tool-under-test")
	writeExecutable(t, checked)
	writeExecutable(t, filepath.Join(root, "bin", "tool-under-test"))
	t.Chdir(cwd)

	got, err := ResolveComponent("tool-under-test", root, Override{Path: "bin/tool-under-test", Source: "TOOL_BIN"})
	if err != nil {
		t.Fatal(err)
	}
	if !filepath.IsAbs(got) {
		t.Fatalf("override handed on relative: %q", got)
	}
	gotReal, err := filepath.EvalSymlinks(got)
	if err != nil {
		t.Fatal(err)
	}
	wantReal, err := filepath.EvalSymlinks(checked)
	if err != nil {
		t.Fatal(err)
	}
	if gotReal != wantReal {
		t.Fatalf("resolved %q, want the file that was checked, %q", gotReal, wantReal)
	}
}
