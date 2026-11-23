package verify_test

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
)

func changedOf(t *testing.T, root string) (verify.GitState, error) {
	t.Helper()
	return verify.CollectGitStateReport(context.Background(), root)
}

// a branch ref file that git ignores as broken: rev-parse and show-ref both
// exit 1 silently, symbolic-ref is fine. Tracked edits must not vanish.
func TestAdversarialBrokenBranchRefDoesNotHideTrackedEdits(t *testing.T) {
	root := committedRepo(t)
	head, err := os.ReadFile(filepath.Join(root, ".git", "HEAD"))
	if err != nil {
		t.Fatal(err)
	}
	ref := strings.TrimSpace(strings.TrimPrefix(string(head), "ref:"))
	if err := os.WriteFile(filepath.Join(root, ".git", ref), []byte("garbage\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "a.txt"), []byte("changed\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	state, err := changedOf(t, root)
	if err == nil && !strings.Contains(strings.Join(state.Changed, ","), "a.txt") {
		t.Fatalf("broken branch ref hid a tracked edit: changed=%v empty=%v", state.Changed, state.Empty)
	}
}

// detached HEAD naming an object that is not there.
func TestAdversarialDetachedHeadMissingObjectFailsClosed(t *testing.T) {
	root := committedRepo(t)
	if err := os.WriteFile(filepath.Join(root, ".git", "HEAD"), []byte(strings.Repeat("ab", 20)+"\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "a.txt"), []byte("changed\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	state, err := changedOf(t, root)
	if err == nil {
		t.Fatalf("detached HEAD at a missing object collected: changed=%v empty=%v", state.Changed, state.Empty)
	}
	if _, err := verify.WorkingTreeSubject(context.Background(), root); err == nil {
		t.Fatal("WorkingTreeSubject built a subject at a missing HEAD object")
	}
}

// corrupt packed-refs with HEAD naming a packed branch.
func TestAdversarialCorruptPackedRefsFailsClosed(t *testing.T) {
	root := committedRepo(t)
	runGitIn(t, root, "pack-refs", "--all")
	if err := os.WriteFile(filepath.Join(root, ".git", "packed-refs"), []byte("not a packed refs file\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "a.txt"), []byte("changed\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	state, err := changedOf(t, root)
	if err == nil && !strings.Contains(strings.Join(state.Changed, ","), "a.txt") {
		t.Fatalf("corrupt packed-refs hid a tracked edit: changed=%v empty=%v", state.Changed, state.Empty)
	}
}

// a linked worktree whose gitdir was removed.
func TestAdversarialWorktreeWithMissingGitdirFailsClosed(t *testing.T) {
	root := committedRepo(t)
	wt := filepath.Join(t.TempDir(), "wt")
	runGitIn(t, root, "worktree", "add", "-q", "-b", "wtb", wt)
	entries, err := os.ReadDir(filepath.Join(root, ".git", "worktrees"))
	if err != nil || len(entries) == 0 {
		t.Fatalf("no worktree admin dir: %v", err)
	}
	if err := os.RemoveAll(filepath.Join(root, ".git", "worktrees", entries[0].Name())); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(wt, "a.txt"), []byte("changed\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	state, err := changedOf(t, wt)
	if err == nil {
		t.Fatalf("worktree with no gitdir collected: changed=%v empty=%v", state.Changed, state.Empty)
	}
}
