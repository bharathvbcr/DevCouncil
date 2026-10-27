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

func nestedClone(t *testing.T, parent, name string) {
	t.Helper()
	dir := filepath.Join(parent, name)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	runGitIn(t, dir, "init", "-q")
	writeFile(t, dir, "inner.txt", "inner\n")
	runGitIn(t, dir, "add", "inner.txt")
	runGitIn(t, dir, "commit", "-qm", "inner")
}

// git lists an un-ignored nested repository as `dir/`. Its files are in no
// listing, so none can be scanned: the change is refused, and the message says
// what to do rather than leaving "not a plain path" to be decoded.
func TestAnUntrackedNestedRepositoryIsRefusedWithAnActionableMessage(t *testing.T) {
	root := gitRepo(t)
	nestedClone(t, root, "vendored")
	_, _, _, err := verify.CollectGitState(context.Background(), root)
	if err == nil {
		t.Fatal("an untracked nested repository was collected without a refusal")
	}
	want := "untracked nested repository vendored/ is not scanned: ignore it or add it as a submodule"
	if !strings.Contains(err.Error(), want) {
		t.Fatalf("refusal = %q, want it to contain %q", err, want)
	}
}

func TestAnUntrackedNestedRepositoryInsideASubmoduleIsRefusedTheSameWay(t *testing.T) {
	root, subDir := submoduleRepo(t)
	nestedClone(t, subDir, "vendored")
	_, _, _, err := verify.CollectGitState(context.Background(), root)
	if err == nil {
		t.Fatal("an untracked nested repository inside a submodule was collected without a refusal")
	}
	want := "untracked nested repository sub/vendored/ is not scanned: ignore it or add it as a submodule"
	if !strings.Contains(err.Error(), want) {
		t.Fatalf("refusal = %q, want it to contain %q", err, want)
	}
}

// A deleted d/x whose parent d is now a file: Lstat says ENOTDIR, which is "not
// there" for a deleted file. The run used to stop on it.
func TestADeletedFileWhoseDirectoryBecameAFileIsDeleted(t *testing.T) {
	root := gitRepo(t)
	writeFile(t, root, "d/x", "one\n")
	attackCommit(t, root, "add d/x")
	if err := os.RemoveAll(filepath.Join(root, "d")); err != nil {
		t.Fatal(err)
	}
	writeFile(t, root, "d", "now a file\n")
	changed, diff, _, err := verify.CollectGitState(context.Background(), root)
	if err != nil {
		t.Fatalf("a deleted file under a directory that became a file stopped the run: %v", err)
	}
	found := map[string]bool{}
	for _, c := range changed {
		found[c] = true
	}
	if !found["d/x"] || !found["d"] {
		t.Fatalf("changed = %q, want both d/x (deleted) and d (the new file)", changed)
	}
	if !strings.Contains(diff, "now a file") {
		t.Fatal("the file that replaced the directory was not read")
	}
}
