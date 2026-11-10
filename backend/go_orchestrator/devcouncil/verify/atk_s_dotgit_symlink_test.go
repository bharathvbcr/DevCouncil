package verify_test

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
)

// Attack round lane S (A3): a tracked symlink whose target is INSIDE .git/.
// The full checkout and the sparse checkout must BOTH refuse, and both refusals
// must name the .git directory (not a raw ENOENT, not an "outside the cone"
// message that would hide the real reason).
func atkSDotGit(t *testing.T, linkPath, target string) {
	t.Helper()
	r := newSubjectRepo(t)
	r.write("services/auth/login.go", "package auth\nfunc Login() {}\n")
	full := filepath.Join(r.root, linkPath)
	if err := os.MkdirAll(filepath.Dir(full), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(target, full); err != nil {
		t.Fatal(err)
	}
	r.run("add", ".")
	r.run("commit", "-qm", "atk-s")

	_, fullErr := verify.WorkingTreeSubject(context.Background(), r.root)

	wt := filepath.Join(t.TempDir(), "sparse-wt")
	r.run("worktree", "add", "--no-checkout", wt, "HEAD")
	runGitAt(t, r.git, wt, "sparse-checkout", "init", "--cone")
	runGitAt(t, r.git, wt, "sparse-checkout", "set", "services/auth")
	runGitAt(t, r.git, wt, "checkout", "HEAD")
	_, sparseErr := verify.WorkingTreeSubject(context.Background(), wt)

	if fullErr == nil || sparseErr == nil {
		t.Fatalf("a symlink into .git must be refused in both: full=%v sparse=%v", fullErr, sparseErr)
	}
	for name, err := range map[string]error{"full": fullErr, "sparse": sparseErr} {
		if !strings.Contains(err.Error(), ".git") {
			t.Fatalf("%s refusal does not name .git: %v", name, err)
		}
	}
	t.Logf("full=%v | sparse=%v", fullErr, sparseErr)
}

func TestAtkSSymlinkIntoDotGitConfigFromOutsideCone(t *testing.T) {
	atkSDotGit(t, "docs/cfg", "../.git/config")
}

func TestAtkSSymlinkIntoDotGitFromInsideCone(t *testing.T) {
	atkSDotGit(t, "services/auth/cfg", "../../.git/config")
}

func TestAtkSSymlinkToDotGitDirectory(t *testing.T) {
	atkSDotGit(t, "docs/g", "../.git")
}

func TestAtkSSymlinkIntoDotGitViaDotDotDetour(t *testing.T) {
	atkSDotGit(t, "docs/cfg", "../services/../.git/HEAD")
}

func TestAtkSSymlinkIntoNestedDotGit(t *testing.T) {
	atkSDotGit(t, "docs/cfg", "../vendor/x/.git/config")
}
