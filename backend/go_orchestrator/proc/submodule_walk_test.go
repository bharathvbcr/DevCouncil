//go:build unix

package proc

import (
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"
)

// repoWithSubmodule is gitRepo with a committed, checked-out submodule `sub`
// holding a clean s.txt.
func repoWithSubmodule(t *testing.T) (root, sub string) {
	t.Helper()
	root = gitRepo(t)
	src := filepath.Join(t.TempDir(), "sub-src")
	if err := os.MkdirAll(src, 0o755); err != nil {
		t.Fatal(err)
	}
	gitRun(t, src, "init", "-q")
	if err := os.WriteFile(filepath.Join(src, "s.txt"), []byte("s\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	gitRun(t, src, "add", "s.txt")
	gitRun(t, src, "-c", "user.email=a@example.test", "-c", "user.name=a", "commit", "-qm", "s")
	gitRun(t, root, "-c", "protocol.file.allow=always", "submodule", "add", "-q", src, "sub")
	gitRun(t, root, "commit", "-qm", "sub")
	return root, filepath.Join(root, "sub")
}

// git calls a file created inside a submodule "modified content"; no listing of
// the superproject names it. The walk has to ask the submodule.
func TestAnUntrackedFileInASubmoduleMakesItDirty(t *testing.T) {
	root, sub := repoWithSubmodule(t)
	session, err := NewGitSession(root)
	if err != nil {
		t.Fatal(err)
	}
	if dirty, err := session.DirtySubmodules(t.Context()); err != nil || len(dirty) != 0 {
		t.Fatalf("a clean submodule was reported dirty: %q, %v", dirty, err)
	}
	if err := os.WriteFile(filepath.Join(sub, "new.txt"), []byte("new\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	dirty, err := session.DirtySubmodules(t.Context())
	if err != nil || !slices.Equal(dirty, []string{"sub"}) {
		t.Fatalf("dirty = %q, %v; want [sub]", dirty, err)
	}
	changes, err := session.SubmoduleWorktreeFiles(t.Context(), "sub")
	if err != nil || !slices.Equal(changes.Files, []string{"sub/new.txt"}) {
		t.Fatalf("changes = %+v, %v; want sub/new.txt", changes, err)
	}
}

// A gitlink in the index that .gitmodules does not name is still a submodule to
// git. Skipping it for want of .gitmodules passed its bytes silently.
func TestAGitlinkWithoutGitmodulesIsStillWalked(t *testing.T) {
	root, sub := repoWithSubmodule(t)
	gitRun(t, root, "rm", "-q", ".gitmodules")
	gitRun(t, root, "commit", "-qm", "drop .gitmodules")
	if err := os.WriteFile(filepath.Join(sub, "s.txt"), []byte("changed\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	session, err := NewGitSession(root)
	if err != nil {
		t.Fatal(err)
	}
	dirty, err := session.DirtySubmodules(t.Context())
	if err != nil || !slices.Equal(dirty, []string{"sub"}) {
		t.Fatalf("dirty = %q, %v; want [sub]", dirty, err)
	}
}

// A directory at a gitlink path that holds files but no .git: no listing names
// those files, so they cannot be read, and the walk must say so.
func TestAGitlinkDirectoryWithFilesAndNoRepositoryIsRefused(t *testing.T) {
	root, sub := repoWithSubmodule(t)
	if err := os.RemoveAll(filepath.Join(sub, ".git")); err != nil {
		t.Fatal(err)
	}
	session, err := NewGitSession(root)
	if err != nil {
		t.Fatal(err)
	}
	dirty, err := session.DirtySubmodules(t.Context())
	if err == nil {
		t.Fatalf("files under a gitlink with no repository were passed unread: dirty=%q", dirty)
	}
	if !strings.Contains(err.Error(), "no git repository") {
		t.Fatalf("the refusal does not say why: %v", err)
	}
}

// A name is exactly what git wrote: a file whose name ends in a space is not
// the file without it.
func TestSubmoduleFileNamesAreNotTrimmed(t *testing.T) {
	root, sub := repoWithSubmodule(t)
	if err := os.WriteFile(filepath.Join(sub, "t.txt "), []byte("one\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	gitRun(t, sub, "add", "-A")
	gitRun(t, sub, "-c", "user.email=a@example.test", "-c", "user.name=a", "commit", "-qm", "t")
	gitRun(t, root, "add", "sub")
	gitRun(t, root, "commit", "-qm", "pointer")
	if err := os.WriteFile(filepath.Join(sub, "t.txt "), []byte("two\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	session, err := NewGitSession(root)
	if err != nil {
		t.Fatal(err)
	}
	changes, err := session.SubmoduleWorktreeFiles(t.Context(), "sub")
	if err != nil {
		t.Fatal(err)
	}
	if !slices.Equal(changes.Files, []string{"sub/t.txt "}) || len(changes.Deleted) != 0 {
		t.Fatalf("changes = %+v; want the one file sub/t.txt (trailing space kept)", changes)
	}
}

// fakeGitScript puts a git on PATH that runs body for every call except
// `version`, which the real git answers, and returns a session using it.
func fakeGitScript(t *testing.T, root, body string) *GitSession {
	t.Helper()
	real, err := exec.LookPath("git")
	if err != nil {
		t.Skip(err)
	}
	bin := t.TempDir()
	script := "#!/bin/sh\nif [ \"$1\" = version ] || [ \"$1\" = --version ]; then exec '" + real + "' \"$@\"; fi\n" + body
	// #nosec G306 -- an executable stand-in; the execute bit is the point.
	if err := os.WriteFile(filepath.Join(bin, "git"), []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", bin+string(os.PathListSeparator)+os.Getenv("PATH"))
	session, err := NewGitSession(root)
	if err != nil {
		t.Fatal(err)
	}
	return session
}

func shortBudget(t *testing.T, d time.Duration) {
	t.Helper()
	old := submoduleWalkBudget
	submoduleWalkBudget = d
	t.Cleanup(func() { submoduleWalkBudget = old })
}

// A git that answers `version` and then never returns on any other call must
// not hold the walk past the walk's own deadline. Each call is bounded at 60 s
// and a walk may make up to 256 of them, so the per-call bound is not it.
func TestTheSubmoduleWalkHasOneOverallDeadline(t *testing.T) {
	root, _ := repoWithSubmodule(t)
	hung := fakeGitScript(t, root, "exec sleep 120\n")
	shortBudget(t, 2*time.Second)

	start := time.Now()
	done := make(chan error, 1)
	go func() {
		_, err := hung.DirtySubmodules(t.Context())
		done <- err
	}()
	select {
	case err := <-done:
		if err == nil {
			t.Fatal("a git that never answered was read as a tree with no dirty submodule")
		}
		if took := time.Since(start); took > 8*time.Second {
			t.Fatalf("the 2s walk deadline was honoured after %s", took)
		}
	case <-time.After(30 * time.Second):
		t.Fatal("the walk ignored its overall deadline for 30s")
	}
}

// Several walk calls made under one WithWalkBudget share it. Each starting its
// own budget made the worst case one budget per call, up to 257 of them.
func TestWalkCallsUnderOneBudgetShareIt(t *testing.T) {
	root, _ := repoWithSubmodule(t)
	hung := fakeGitScript(t, root, "exec sleep 120\n")
	shortBudget(t, 2*time.Second)

	ctx, cancel := WithWalkBudget(t.Context())
	defer cancel()
	start := time.Now()
	done := make(chan struct{})
	go func() {
		defer close(done)
		for i := 0; i < 3; i++ {
			if _, err := hung.DirtySubmodules(ctx); err == nil {
				t.Errorf("call %d: a git that never answered was read as clean", i)
			}
		}
	}()
	select {
	case <-done:
		if took := time.Since(start); took > 4500*time.Millisecond {
			t.Fatalf("three calls under one 2s budget took %s: each started its own", took)
		}
	case <-time.After(30 * time.Second):
		t.Fatal("the calls ignored the shared budget for 30s")
	}
}

// An index of some 800 000 files, none of them a gitlink, is not refused for
// its size: the listing is filtered as it streams, and only gitlink records are
// kept. (A total cap of 64 MiB failed it.) The listing here is about 75 MiB.
func TestALargeIndexWithNoSubmoduleIsNotRefused(t *testing.T) {
	root, _ := repoWithSubmodule(t)
	if _, err := exec.LookPath("perl"); err != nil {
		t.Skip(err)
	}
	session := fakeGitScript(t, root, `case "$*" in
*ls-files*--stage*)
  exec perl -e 'for my $i (0..999999) { print "100644 0123456789abcdef0123456789abcdef01234567 0\tdirectory/file-$i.txt\0" } print "160000 0123456789abcdef0123456789abcdef01234567 0\tabsent\0"'
  ;;
esac
exec `+"'"+mustLookGit(t, root)+"'"+` "$@"
`)
	dirty, err := session.DirtySubmodules(t.Context())
	if err != nil {
		t.Fatalf("a large index with no checked-out submodule was refused: %v", err)
	}
	if len(dirty) != 0 {
		t.Fatalf("dirty = %q, want none (the one gitlink is not checked out)", dirty)
	}
}

// A record that never ends is refused, not buffered whole.
func TestAnIndexRecordWithoutAnEndIsRefused(t *testing.T) {
	root, _ := repoWithSubmodule(t)
	if _, err := exec.LookPath("perl"); err != nil {
		t.Skip(err)
	}
	session := fakeGitScript(t, root, `case "$*" in
*ls-files*--stage*)
  exec perl -e 'print "100644 0123456789abcdef0123456789abcdef01234567 0\t" . ("x" x (4 << 20))'
  ;;
esac
exec `+"'"+mustLookGit(t, root)+"'"+` "$@"
`)
	if _, err := session.DirtySubmodules(t.Context()); err == nil {
		t.Fatal("a 4 MiB record was accepted")
	}
}
