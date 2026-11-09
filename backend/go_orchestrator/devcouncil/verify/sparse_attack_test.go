package verify_test

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
)

// Sparse and full subjects must agree for blob-sharing variants beyond
// file+symlink. The "gitlink" variant is a real `git submodule add` fixture:
// the earlier bare `update-index --cacheinfo` gitlink had no .gitmodules and no
// repository behind it, which is not a state git itself produces.
func TestSparseAgreesWithFullOnSharedBlobVariants(t *testing.T) {
	for _, v := range []string{"exec", "twin", "symlink", "duplicate-symlinks", "gitlink", "gitlink-emptydir"} {
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
	case "duplicate-symlinks":
		r.write("services/billing/x/a.txt", "same\n")
		r.write("services/billing/y/a.txt", "same\n")
		if err := os.Symlink("a.txt", filepath.Join(r.root, "services/billing/x/l1")); err != nil {
			t.Fatal(err)
		}
		if err := os.Symlink("a.txt", filepath.Join(r.root, "services/billing/y/l2")); err != nil {
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

func TestSparseRefusesSharedBlobSymlinkEscape(t *testing.T) {
	r := newSubjectRepo(t)
	r.write("services/auth/login.go", "package auth\nfunc Login() {}\n")
	r.write("services/billing/inside.txt", "ok\n")
	if err := os.MkdirAll(filepath.Join(r.root, "services/billing/a/b/c"), 0o755); err != nil {
		t.Fatal(err)
	}
	// Both symlinks share target "../../../inside.txt" -> identical (oid, 120000) key.
	// The deep one stays inside services/billing/inside.txt; the shallow one escapes repo root.
	if err := os.Symlink("../../../inside.txt", filepath.Join(r.root, "services/billing/a/b/c/ok")); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink("../../../inside.txt", filepath.Join(r.root, "services/billing/escape")); err != nil {
		t.Fatal(err)
	}
	r.run("add", ".")
	r.run("commit", "-qm", "shared-symlink-escape")

	if _, err := verify.WorkingTreeSubject(context.Background(), r.root); err == nil {
		t.Fatal("full checkout should refuse escaping symlink")
	}

	wt := filepath.Join(t.TempDir(), "sparse-escape-wt")
	r.run("worktree", "add", "--no-checkout", wt, "HEAD")
	runGitAt(t, r.git, wt, "sparse-checkout", "init", "--cone")
	runGitAt(t, r.git, wt, "sparse-checkout", "set", "services/auth")
	runGitAt(t, r.git, wt, "checkout", "HEAD")

	_, err := verify.WorkingTreeSubject(context.Background(), wt)
	if err == nil {
		t.Fatal("sparse checkout accepted shared-OID escaping symlink")
	}
	if errors.Is(err, os.ErrNotExist) {
		t.Fatalf("sparse checkout failed with ENOENT instead of lexical symlink refusal: %v", err)
	}
	if !strings.Contains(err.Error(), "outside the repository") && !strings.Contains(err.Error(), "escapes") {
		t.Fatalf("expected outside-repository symlink refusal, got: %v", err)
	}
}
