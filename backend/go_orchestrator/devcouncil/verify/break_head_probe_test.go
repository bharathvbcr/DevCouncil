package verify_test

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
)

var _ = verify.CollectGitStateReport

// orphan branch in a repo that has commits elsewhere: git checkout --orphan
// keeps the previous branch's files staged. Those staged files are the agent's
// additions relative to an empty HEAD and must be listed.
func TestAdversarialOrphanBranchListsStagedFiles(t *testing.T) {
	root := committedRepo(t)
	runGitIn(t, root, "checkout", "-q", "--orphan", "fresh")
	if err := os.WriteFile(filepath.Join(root, "b.txt"), []byte("b\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	runGitIn(t, root, "add", "b.txt")
	state, err := changedOf(t, root)
	if err != nil {
		t.Fatal(err)
	}
	joined := strings.Join(state.Changed, ",")
	if !strings.Contains(joined, "b.txt") || !strings.Contains(joined, "a.txt") {
		t.Fatalf("orphan branch with staged files: changed=%v empty=%v diff=%q", state.Changed, state.Empty, state.Diff)
	}
}

// GIT_DIR / GIT_WORK_TREE from the caller must not redirect the reads to
// another repository.
func TestAdversarialCallerGitDirDoesNotRedirect(t *testing.T) {
	root := committedRepo(t)
	other := committedRepo(t)
	if err := os.WriteFile(filepath.Join(root, "a.txt"), []byte("changed\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	t.Setenv("GIT_DIR", filepath.Join(other, ".git"))
	t.Setenv("GIT_WORK_TREE", other)
	state, err := changedOf(t, root)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(strings.Join(state.Changed, ","), "a.txt") {
		t.Fatalf("caller GIT_DIR redirected the read: changed=%v empty=%v", state.Changed, state.Empty)
	}
}
