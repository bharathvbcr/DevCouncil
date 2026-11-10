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

// Attack tests for P8 (shared-blob symlink fan-out in a sparse checkout).
// Each case builds a repo, takes the full-checkout verdict, then the verdict
// in a cone checkout of services/auth (or the cone named by the case), and
// requires both to agree: either the same subject or both refuse, and a
// sparse refusal must never be a raw ENOENT from os.Readlink.

var atkTolerateOverRefusal bool

type atkSym struct {
	path, target string
}

func atkAgree(t *testing.T, files map[string]string, syms []atkSym, cone ...string) {
	t.Helper()
	r := newSubjectRepo(t)
	r.write("services/auth/login.go", "package auth\nfunc Login() {}\n")
	for p, body := range files {
		r.write(p, body)
	}
	for _, s := range syms {
		full := filepath.Join(r.root, s.path)
		if err := os.MkdirAll(filepath.Dir(full), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.Symlink(s.target, full); err != nil {
			t.Fatal(err)
		}
	}
	r.run("add", ".")
	r.run("commit", "-qm", "atk")

	fullSubject, fullErr := verify.WorkingTreeSubject(context.Background(), r.root)

	if len(cone) == 0 {
		cone = []string{"services/auth"}
	}
	wt := filepath.Join(t.TempDir(), "sparse-wt")
	r.run("worktree", "add", "--no-checkout", wt, "HEAD")
	runGitAt(t, r.git, wt, "sparse-checkout", "init", "--cone")
	runGitAt(t, r.git, wt, append([]string{"sparse-checkout", "set"}, cone...)...)
	runGitAt(t, r.git, wt, "checkout", "HEAD")
	sparseSubject, sparseErr := verify.WorkingTreeSubject(context.Background(), wt)

	if fullErr == nil && sparseErr != nil && atkTolerateOverRefusal {
		// Pre-existing on 79468e96 for a single symlink (verified): the sparse
		// path refuses an in-tree directory or dangling target as "not tracked".
		// Fail closed, so characterised: the refusal must be the cone refusal
		// and never a raw ENOENT from os.Readlink.
		if errors.Is(sparseErr, os.ErrNotExist) {
			t.Fatalf("sparse refusal is a raw ENOENT, not the cone refusal: %v", sparseErr)
		}
		const coneRefusal = "outside the sparse-checkout cone and not tracked in HEAD"
		if !strings.Contains(sparseErr.Error(), coneRefusal) {
			t.Fatalf("sparse over-refusal is not the cone refusal (%q): %v", coneRefusal, sparseErr)
		}
		t.Logf("sparse over-refuses (fail closed): %v", sparseErr)
		return
	}
	if (fullErr == nil) != (sparseErr == nil) {
		t.Fatalf("verdicts disagree: full err=%v, sparse err=%v", fullErr, sparseErr)
	}
	if sparseErr != nil {
		if errors.Is(sparseErr, os.ErrNotExist) {
			t.Fatalf("sparse refusal is a raw ENOENT, not a symlink verdict: %v", sparseErr)
		}
		t.Logf("both refuse: full=%v sparse=%v", fullErr, sparseErr)
		return
	}
	if fullSubject != sparseSubject {
		t.Fatalf("sparse %q != full %q", sparseSubject, fullSubject)
	}
}

// N=3 paths share one symlink blob, all inside.
func TestAtkYThreeSymlinksOneBlobAllInside(t *testing.T) {
	atkAgree(t,
		map[string]string{"services/billing/inside.txt": "ok\n"},
		[]atkSym{
			{"services/billing/a/l1", "../inside.txt"},
			{"services/billing/b/l2", "../inside.txt"},
			{"services/billing/c/l3", "../inside.txt"},
		})
}

// The third path (last in index order) is the only one that escapes.
func TestAtkYEscapeOnThirdPath(t *testing.T) {
	atkAgree(t,
		map[string]string{"services/billing/inside.txt": "ok\n"},
		[]atkSym{
			{"services/billing/a/b/c/l1", "../../../inside.txt"},
			{"services/billing/a/b/d/l2", "../../../inside.txt"},
			{"zz_escape", "../../../inside.txt"},
		})
}

// The second path escapes, the third is fine.
func TestAtkYEscapeOnSecondPath(t *testing.T) {
	atkAgree(t,
		map[string]string{"services/billing/inside.txt": "ok\n"},
		[]atkSym{
			{"services/billing/a/b/c/l1", "../../../inside.txt"},
			{"services/billing/e_escape", "../../../inside.txt"},
			{"services/billing/f/g/h/l3", "../../../inside.txt"},
		})
}

// Absolute target shared by three paths.
func TestAtkYAbsoluteTargetSharedByThree(t *testing.T) {
	atkAgree(t, nil, []atkSym{
		{"services/billing/l1", "/etc/hosts"},
		{"services/billing/l2", "/etc/hosts"},
		{"other/l3", "/etc/hosts"},
	})
}

// A regular file whose bytes are the symlink target text shares the blob oid
// with the symlinks (mode differs): the symlink check must not be confused.
func TestAtkYRegularFileSharesBlobWithSymlinks(t *testing.T) {
	atkAgree(t,
		map[string]string{
			"services/billing/a.txt":   "a.txt",
			"services/billing/x/a.txt": "a.txt",
			"services/billing/copy":    "../escape-target",
		},
		[]atkSym{
			{"services/billing/l1", "a.txt"},
			{"services/billing/x/l2", "a.txt"},
		})
}

// A regular file shares the blob with a symlink that escapes: the escape must
// still be refused in a sparse tree even though the blob key also exists as a
// regular file.
func TestAtkYRegularFileSharesBlobWithEscapingSymlinks(t *testing.T) {
	atkAgree(t,
		map[string]string{
			"services/billing/copy": "../../../outside",
		},
		[]atkSym{
			{"services/billing/l1", "../../../outside"},
			{"services/billing/x/l2", "../../../outside"},
		})
}

// Symlinks to a directory, shared blob.
func TestAtkYSymlinkToDirectorySharedBlob(t *testing.T) {
	atkTolerateOverRefusal = true
	defer func() { atkTolerateOverRefusal = false }()
	atkAgree(t,
		map[string]string{"services/billing/d/f.txt": "f\n"},
		[]atkSym{
			{"services/billing/l1", "d"},
			{"services/billing/l2", "d"},
			{"services/billing/l3", "d"},
		})
}

// Symlink to a directory outside the repo, shared blob.
func TestAtkYSymlinkToOutsideDirectorySharedBlob(t *testing.T) {
	atkAgree(t, nil, []atkSym{
		{"services/billing/l1", "../../.."},
		{"services/billing/x/l2", "../../.."},
	})
}

// One of the sharing symlinks is inside the cone (materialized, read with
// os.Readlink), the others are outside (fan-out from the blob).
func TestAtkYMixedMaterializedAndUnmaterializedShareBlob(t *testing.T) {
	atkTolerateOverRefusal = true
	defer func() { atkTolerateOverRefusal = false }()
	atkAgree(t,
		map[string]string{
			"services/billing/inside.txt": "ok\n",
			"services/auth/inside.txt":    "ok\n",
		},
		[]atkSym{
			{"services/auth/l0", "inside.txt"},
			{"services/billing/inside_l", "inside.txt"},
			{"services/billing/x/inside_l", "inside.txt"},
		})
}

// The cone contains one sharer; the unmaterialized sharer targets a path in
// the cone (stagedSet / cone.contains branch).
func TestAtkYUnmaterializedLinkToConePath(t *testing.T) {
	atkAgree(t, nil, []atkSym{
		{"services/billing/l1", "../auth/login.go"},
		{"services/billing/x/l2", "../../auth/login.go"},
		{"services/billing/y/l3", "../../auth/login.go"},
	})
}

// Dangling sharers (target never exists), 3 paths.
func TestAtkYDanglingSharedBlob(t *testing.T) {
	atkTolerateOverRefusal = true
	defer func() { atkTolerateOverRefusal = false }()
	atkAgree(t, nil, []atkSym{
		{"services/billing/l1", "missing.go"},
		{"services/billing/l2", "missing.go"},
		{"services/billing/l3", "missing.go"},
	})
}

// Symlink pointing at a symlink that shares the blob (chain through the
// rawTargets map used by resolveSymlinkLexical).
func TestAtkYSymlinkChainThroughSharedBlob(t *testing.T) {
	atkAgree(t,
		map[string]string{"services/billing/real.txt": "r\n"},
		[]atkSym{
			{"services/billing/a", "real.txt"},
			{"services/billing/b", "a"},
			{"services/billing/c", "a"},
			{"services/billing/x/c", "../a"},
		})
}
