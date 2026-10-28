//go:build unix

package proc

import (
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
)

// A raw record that does not parse is an error, never a shorter list. (Salvaged
// from the range reader's test of the same filter; the reader is not carried.)
func TestMovedGitlinkFilterRefusesWhatItCannotParse(t *testing.T) {
	good := ":160000 160000 " + strings.Repeat("a", 40) + " " + strings.Repeat("0", 40) + " M\x00sub\x00"
	cases := map[string]string{
		"header with four fields": ":160000 160000 " + strings.Repeat("a", 40) + " M\x00sub\x00",
		"header without a colon":  "160000 160000 a b M\x00sub\x00",
		"header with no status":   ":160000 160000 a b \x00sub\x00",
		"record with no path":     ":160000 160000 a b M\x00",
		"rename with one path":    ":160000 160000 a b R100\x00old\x00",
		"empty path":              ":160000 160000 a b M\x00\x00",
		"cut inside a path":       good + ":160000 160000 a b M\x00su",
		"garbage":                 "not a raw record\x00",
	}
	for name, input := range cases {
		t.Run(name, func(t *testing.T) {
			f := &movedGitlinkFilter{}
			if _, err := f.Write([]byte(input)); err != nil {
				t.Fatal(err)
			}
			if err := f.finish("root"); err == nil {
				t.Fatalf("%q parsed as %+v", input, f.moved)
			}
		})
	}
	t.Run("a well-formed listing parses, across writes", func(t *testing.T) {
		input := ":100644 100644 a b M\x00README\x00" + good +
			":000000 160000 " + strings.Repeat("0", 40) + " " + strings.Repeat("c", 40) + " A\x00fresh\x00" +
			":160000 000000 a b D\x00gone\x00" +
			":100644 160000 a b T\x00in\x00" +
			":160000 100644 a b T\x00out\x00" +
			":160000 160000 a b R100\x00old\x00renamed\x00"
		f := &movedGitlinkFilter{}
		for i := 0; i < len(input); i++ {
			if n, err := f.Write([]byte{input[i]}); n != 1 || err != nil {
				t.Fatalf("Write = %d, %v", n, err)
			}
		}
		if err := f.finish("root"); err != nil {
			t.Fatal(err)
		}
		want := []GitlinkChange{
			{"sub", GitlinkMoved}, {"fresh", GitlinkAdded}, {"gone", GitlinkDeleted},
			{"in", GitlinkFileToSubmodule}, {"out", GitlinkSubmoduleToFile}, {"renamed", GitlinkMoved},
		}
		if !reflect.DeepEqual(f.moved, want) {
			t.Fatalf("moved = %+v, want %+v", f.moved, want)
		}
	})
	t.Run("a record past the entry bound", func(t *testing.T) {
		f := &movedGitlinkFilter{}
		if _, err := f.Write([]byte(strings.Repeat("x", maxIndexEntry+1))); err != nil {
			t.Fatal(err)
		}
		if err := f.finish("root"); err == nil {
			t.Fatal("an overlong record was accepted")
		}
	})
	t.Run("more gitlinks than one read examines", func(t *testing.T) {
		f := &movedGitlinkFilter{}
		for i := 0; i <= maxSubmodules; i++ {
			rec := ":160000 160000 a b M\x00" + strings.Repeat("p", 1) + string(rune('a'+i%26)) + string(rune('a'+i/26%26)) + "\x00"
			if _, err := f.Write([]byte(rec)); err != nil {
				t.Fatal(err)
			}
		}
		if err := f.finish("root"); err == nil {
			t.Fatalf("%d moved gitlinks were accepted", maxSubmodules+1)
		}
		if len(f.moved) > maxSubmodules {
			t.Fatalf("kept %d, more than the bound", len(f.moved))
		}
	})
}

// subCommit adds one file to the repository at dir and commits it.
func subCommit(t *testing.T, dir, name, body, msg string) {
	t.Helper()
	if err := os.WriteFile(filepath.Join(dir, name), []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
	gitRun(t, dir, "add", name)
	gitRun(t, dir, "-c", "user.email=a@example.test", "-c", "user.name=a", "commit", "-qm", msg)
}

func gitlinkChanges(t *testing.T, root string, headless bool) []GitlinkChange {
	t.Helper()
	session, err := NewGitSession(root)
	if err != nil {
		t.Fatal(err)
	}
	moved, err := session.MovedGitlinks(t.Context(), headless)
	if err != nil {
		t.Fatalf("MovedGitlinks: %v", err)
	}
	return moved
}

// Every shape a gitlink change can take is listed with its shape, and a gitlink
// that did not change is not.
func TestMovedGitlinksNamesEveryShape(t *testing.T) {
	root, sub := repoWithSubmodule(t)
	if got := gitlinkChanges(t, root, false); len(got) != 0 {
		t.Fatalf("an unchanged gitlink was listed: %+v", got)
	}
	subCommit(t, sub, "later.txt", "later\n", "later")
	if got, want := gitlinkChanges(t, root, false), []GitlinkChange{{"sub", GitlinkMoved}}; !reflect.DeepEqual(got, want) {
		t.Fatalf("moved: got %+v, want %+v", got, want)
	}
	gitRun(t, root, "rm", "-q", "-f", "sub")
	if got, want := gitlinkChanges(t, root, false), []GitlinkChange{{"sub", GitlinkDeleted}}; !reflect.DeepEqual(got, want) {
		t.Fatalf("deleted: got %+v, want %+v", got, want)
	}
}

func TestMovedGitlinksNamesTypechangesInBothDirections(t *testing.T) {
	root, _ := repoWithSubmodule(t)
	// A submodule replaced by a file.
	gitRun(t, root, "rm", "-q", "--cached", "sub")
	if err := os.RemoveAll(filepath.Join(root, "sub")); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "sub"), []byte("now a file\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	gitRun(t, root, "add", "sub")
	if got, want := gitlinkChanges(t, root, false), []GitlinkChange{{"sub", GitlinkSubmoduleToFile}}; !reflect.DeepEqual(got, want) {
		t.Fatalf("submodule to file: got %+v, want %+v", got, want)
	}
	// A file replaced by a submodule.
	other, _ := repoWithSubmodule(t)
	if err := os.WriteFile(filepath.Join(other, "plain"), []byte("a file\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	gitRun(t, other, "add", "plain")
	gitRun(t, other, "-c", "user.email=a@example.test", "-c", "user.name=a", "commit", "-qm", "add plain")
	gitRun(t, other, "rm", "-q", "-f", "plain")
	cmd := exec.Command("git", "-c", "protocol.file.allow=always", "submodule", "add", "-q", filepath.Join(other, "sub"), "plain")
	cmd.Dir = other
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("submodule add: %v %s", err, out)
	}
	if got, want := gitlinkChanges(t, other, false), []GitlinkChange{{"plain", GitlinkFileToSubmodule}}; !reflect.DeepEqual(got, want) {
		t.Fatalf("file to submodule: got %+v, want %+v", got, want)
	}
}

// `ignore = all` hides a submodule from plain `git diff` and from `status`.
// The listing is not a plain diff.
func TestMovedGitlinksIgnoreAllDoesNotHideAMovedSubmodule(t *testing.T) {
	root, sub := repoWithSubmodule(t)
	gitRun(t, root, "config", "-f", ".gitmodules", "submodule.sub.ignore", "all")
	gitRun(t, root, "config", "diff.ignoreSubmodules", "all")
	gitRun(t, root, "config", "submodule.sub.ignore", "all")
	gitRun(t, root, "add", ".gitmodules")
	gitRun(t, root, "-c", "user.email=a@example.test", "-c", "user.name=a", "commit", "-qm", "ignore all")
	subCommit(t, sub, "later.txt", "later\n", "later")
	// The control: a plain diff in the same repository prints nothing, or this
	// test proves nothing.
	if got := gitOutput(t, root, "diff", "HEAD", "--raw"); strings.Contains(got, gitlinkMode) {
		t.Skipf("this git lists the moved submodule despite ignore = all: %q", got)
	}
	if got, want := gitlinkChanges(t, root, false), []GitlinkChange{{"sub", GitlinkMoved}}; !reflect.DeepEqual(got, want) {
		t.Fatalf("got %+v, want %+v", got, want)
	}
}

// A repository with no HEAD has nothing to compare against: each gitlink in it
// is new.
func TestMovedGitlinksHeadlessListsEveryGitlinkAsAdded(t *testing.T) {
	src, _ := repoWithSubmodule(t)
	root := t.TempDir()
	gitRun(t, root, "init", "-q")
	cmd := exec.Command("git", "-c", "protocol.file.allow=always", "submodule", "add", "-q", filepath.Join(src, "sub"), "mod")
	cmd.Dir = root
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("submodule add: %v %s", err, out)
	}
	if got, want := gitlinkChanges(t, root, true), []GitlinkChange{{"mod", GitlinkAdded}}; !reflect.DeepEqual(got, want) {
		t.Fatalf("got %+v, want %+v", got, want)
	}
}

// Enumeration opens no object in the submodule: a move to a commit the
// submodule no longer holds the bytes of is listed all the same.
func TestMovedGitlinksReadsNothingInsideTheSubmodule(t *testing.T) {
	root, sub := repoWithSubmodule(t)
	subCommit(t, sub, "later.txt", "later\n", "later")
	head := strings.TrimSpace(gitOutput(t, sub, "rev-parse", "HEAD"))
	gitDir := strings.TrimSpace(gitOutput(t, sub, "rev-parse", "--absolute-git-dir"))
	if err := os.Remove(filepath.Join(gitDir, "objects", head[:2], head[2:])); err != nil {
		t.Skipf("the commit is not a loose object: %v", err)
	}
	if got, want := gitlinkChanges(t, root, false), []GitlinkChange{{"sub", GitlinkMoved}}; !reflect.DeepEqual(got, want) {
		t.Fatalf("got %+v, want %+v", got, want)
	}
}
