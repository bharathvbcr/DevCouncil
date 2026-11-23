//go:build !windows

package verify_test

import (
	"context"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
)

// The exclude that keeps tool state out of the diff the gates judge and the read
// that scans it must agree on case, as policy.IsToolState folds it. A tracked
// `.DevMap/` file is tool state to the policy and the read; leaving it in the
// scope diff meant the same path was judged by one view and exempt in another.
// ft-c397c278.
func TestToolStateExcludeFoldsCaseLikeTheRead(t *testing.T) {
	for _, tc := range []struct {
		name   string
		staged bool
	}{{"worktree change", false}, {"staged change", true}} {
		t.Run(tc.name, func(t *testing.T) {
			root := gitRepo(t)
			writeFile(t, root, ".DevMap/k.txt", "one\n")
			writeFile(t, root, "src/a.txt", "a\n")
			attackCommit(t, root, "track")
			writeFile(t, root, ".DevMap/k.txt", "one\nmarker-two\n")
			writeFile(t, root, "src/a.txt", "a\nreal-change\n")
			if tc.staged {
				runGitIn(t, root, "add", ".DevMap/k.txt", "src/a.txt")
			}
			state, err := verify.CollectGitStateReport(context.Background(), root)
			if err != nil {
				t.Fatal(err)
			}
			if strings.Contains(state.Diff, "marker-two") || strings.Contains(state.Diff, ".DevMap") {
				t.Fatalf("a differently-cased tool-state path is in the scope diff:\n%s", state.Diff)
			}
			if !strings.Contains(state.Diff, "real-change") {
				t.Fatalf("an ordinary change is missing from the diff:\n%s", state.Diff)
			}
		})
	}
}

// A file staged and left unchanged on disk reads the same in the index view and
// the worktree view; its section is carried once, not twice.
func TestStagedChangeIsNotDuplicatedInTheDiff(t *testing.T) {
	root := gitRepo(t)
	writeFile(t, root, "src/a.txt", "a\n")
	attackCommit(t, root, "track")
	writeFile(t, root, "src/a.txt", "a\nonly-once\n")
	runGitIn(t, root, "add", "src/a.txt")
	state, err := verify.CollectGitStateReport(context.Background(), root)
	if err != nil {
		t.Fatal(err)
	}
	if n := strings.Count(state.Diff, "only-once"); n != 1 {
		t.Fatalf("staged line appears %d times in the diff:\n%s", n, state.Diff)
	}
}
