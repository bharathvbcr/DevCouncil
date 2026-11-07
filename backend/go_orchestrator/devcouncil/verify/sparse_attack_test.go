package verify_test

import (
	"context"
	"os"
	"path/filepath"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
)

// Sparse and full subjects must agree for blob-sharing variants beyond
// file+symlink. The "gitlink" variant is a real `git submodule add` fixture:
// the earlier bare `update-index --cacheinfo` gitlink had no .gitmodules and no
// repository behind it, which is not a state git itself produces.
func TestSparseAgreesWithFullOnSharedBlobVariants(t *testing.T) {
	for _, v := range []string{"exec", "twin", "symlink", "gitlink", "gitlink-emptydir"} {
		v := v
		t.Run(v, func(t *testing.T) { attack11(t, v) })
	}
}

func attack11(t *testing.T, variant string) {
	r := newSubjectRepo(t)
	r.write("services/auth/login.go", "package auth\nfunc Login() {}\n")
	switch variant {
	case "exec":
		r.write("services/billing/x/plain.sh", "echo hi\n")
		r.write("services/billing/x/exec.sh", "echo hi\n")
		if err := os.Chmod(filepath.Join(r.root, "services/billing/x/exec.sh"), 0o755); err != nil {
			t.Fatal(err)
		}
	case "twin":
		r.write("services/billing/x/one.sh", "echo hi\n")
		r.write("services/billing/x/two.sh", "echo hi\n")
	case "symlink":
		r.write("services/billing/x/a.txt", "a.txt")
		if err := os.Symlink("a.txt", filepath.Join(r.root, "services/billing/x/l")); err != nil {
			t.Fatal(err)
		}
	}
	r.run("add", ".")
	if variant == "gitlink" {
		sub := filepath.Join(t.TempDir(), "sub")
		if err := os.MkdirAll(sub, 0o755); err != nil {
			t.Fatal(err)
		}
		runGitAt(t, r.git, sub, "init", "-q")
		if err := os.WriteFile(filepath.Join(sub, "f.txt"), []byte("sub\n"), 0o644); err != nil {
			t.Fatal(err)
		}
		runGitAt(t, r.git, sub, "add", ".")
		runGitAt(t, r.git, sub, "commit", "-qm", "sub")
		r.run("-c", "protocol.file.allow=always", "submodule", "add", sub, "services/billing/sub")
	}
	if variant == "gitlink-emptydir" {
		r.run("update-index", "--add", "--cacheinfo", "160000,1111111111111111111111111111111111111111,services/billing/sub")
	}
	if variant == "gitlink-emptydir" {
		if err := os.MkdirAll(filepath.Join(r.root, "services/billing/sub"), 0o755); err != nil {
			t.Fatal(err)
		}
	}
	r.run("commit", "-qm", "variants")

	full := r.subject()

	wt := filepath.Join(t.TempDir(), "sparse-wt")
	r.run("worktree", "add", "--no-checkout", wt, "HEAD")
	runGitAt(t, r.git, wt, "sparse-checkout", "init", "--cone")
	runGitAt(t, r.git, wt, "sparse-checkout", "set", "services/auth")
	runGitAt(t, r.git, wt, "checkout", "HEAD")
	sparse, err := verify.WorkingTreeSubject(context.Background(), wt)
	if err != nil {
		t.Fatalf("sparse WorkingTreeSubject: %v", err)
	}
	if sparse != full {
		t.Fatalf("sparse %q != full %q", sparse, full)
	}
}
