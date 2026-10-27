//go:build unix

package verify_test

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
)

// Names git quotes in a `diff --git` header, or that hold " b/", used to be
// misread when the supplement decided which changed paths the diff already
// carried. A deleted one then failed Lstat and blocked verification, and a
// modified one was scanned twice. (A quote is not a legal Windows filename,
// hence the unix build tag.)
func TestDiffPresenceDoesNotParseHeaderNames(t *testing.T) {
	root := gitRepo(t)
	deletedSpaced := "x b/gone.txt"
	deletedQuoted := "q\"uote.txt"
	modified := "m b/mod.txt"
	writeFile(t, root, deletedSpaced, "gone\n")
	writeFile(t, root, deletedQuoted, "gone too\n")
	writeFile(t, root, modified, "before\n")
	runGitIn(t, root, "add", ".")
	runGitIn(t, root, "commit", "-qm", "names")

	for _, rel := range []string{deletedSpaced, deletedQuoted} {
		if err := os.Remove(filepath.Join(root, filepath.FromSlash(rel))); err != nil {
			t.Fatal(err)
		}
	}
	marker := "UNIQUE-MODIFIED-LINE"
	writeFile(t, root, modified, marker+"\n")
	// An untracked file that shares the tail of the modified name: a header
	// parsed on the last " b/" reads "mod.txt" out of
	// `a/m b/mod.txt b/m b/mod.txt` and would call this file already present.
	writeFile(t, root, "mod.txt", "untracked and clean of secrets\n")

	changed, diff, empty, err := verify.CollectGitState(context.Background(), root)
	if err != nil {
		t.Fatalf("a deleted file with a quoted or spaced name blocked collection: %v", err)
	}
	if empty {
		t.Fatal("collected as an empty tree")
	}
	if n := strings.Count(diff, marker); n != 1 {
		t.Fatalf("the modified file's added line appears %d times in the diff, want 1 (scanned twice or not at all)", n)
	}
	if !strings.Contains(diff, "untracked and clean of secrets") {
		t.Fatal("the untracked file whose name is a suffix of a changed name was treated as already present")
	}
	for _, want := range []string{deletedSpaced, deletedQuoted, modified, "mod.txt"} {
		found := false
		for _, c := range changed {
			if c == want {
				found = true
			}
		}
		if !found {
			t.Fatalf("changed paths %q lack %q", changed, want)
		}
	}
}
