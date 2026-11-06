package verify_test

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
)

func runGitAt(t *testing.T, gitBin, dir string, args ...string) {
	t.Helper()
	cmd := exec.Command(gitBin, args...)
	cmd.Dir = dir
	cmd.Env = append(os.Environ(),
		"GIT_AUTHOR_NAME=t",
		"GIT_AUTHOR_EMAIL=t@e.invalid",
		"GIT_COMMITTER_NAME=t",
		"GIT_COMMITTER_EMAIL=t@e.invalid",
	)
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("git -C %s %v: %v\n%s", dir, args, err, out)
	}
}

func TestCleanSparseConeWorktreeMatchesFullSubjectBitForBit(t *testing.T) {
	r := newSubjectRepo(t)
	r.write("services/doc.go", "package services\n")
	r.write("services/auth/login.go", "package auth\nfunc Login() {}\n")

	authRun := filepath.Join(r.root, "services/auth/run.sh")
	if err := os.WriteFile(authRun, []byte("#!/bin/sh\necho auth\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	r.write("services/billing/invoice.go", "package billing\nfunc Invoice() {}\n")
	billPay := filepath.Join(r.root, "services/billing/pay.sh")
	if err := os.WriteFile(billPay, []byte("#!/bin/sh\necho pay\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink("invoice.go", filepath.Join(r.root, "services/billing/alias.go")); err != nil {
		t.Fatal(err)
	}
	r.run("add", ".")
	r.run("commit", "-qm", "monorepo layout")

	fullSubject := r.subject()

	wt := filepath.Join(t.TempDir(), "sparse-wt")
	r.run("worktree", "add", "--no-checkout", wt, "HEAD")
	runGitAt(t, r.git, wt, "sparse-checkout", "init", "--cone")
	runGitAt(t, r.git, wt, "sparse-checkout", "set", "services/auth")
	runGitAt(t, r.git, wt, "checkout", "HEAD")

	if _, err := os.Lstat(filepath.Join(wt, "services/billing/invoice.go")); !os.IsNotExist(err) {
		t.Fatalf("expected out-of-cone file to remain unmaterialized, got err=%v", err)
	}

	sparseSubject, err := verify.WorkingTreeSubject(context.Background(), wt)
	if err != nil {
		t.Fatalf("sparse cone WorkingTreeSubject failed: %v", err)
	}
	if sparseSubject != fullSubject {
		t.Fatalf("sparse cone subject %q != full checkout subject %q", sparseSubject, fullSubject)
	}

	// Attack 1: Delete an in-cone file on disk AND tamper with wt's real index
	// SKIP_WORKTREE bit so `git status` hides the deletion. WorkingTreeSubject
	// must NOT trust `.git/index` and must move the subject.
	if err := os.Remove(filepath.Join(wt, "services/auth/login.go")); err != nil {
		t.Fatal(err)
	}
	runGitAt(t, r.git, wt, "update-index", "--skip-worktree", "services/auth/login.go")
	afterTamper, err := verify.WorkingTreeSubject(context.Background(), wt)
	if err != nil {
		t.Fatalf("WorkingTreeSubject after in-cone deletion: %v", err)
	}
	if afterTamper == fullSubject {
		t.Fatal("WorkingTreeSubject collided with clean subject despite deleted in-cone file hidden via SKIP_WORKTREE")
	}

	// Restore in-cone file and verify clean parity returns.
	runGitAt(t, r.git, wt, "update-index", "--no-skip-worktree", "services/auth/login.go")
	runGitAt(t, r.git, wt, "checkout", "--", "services/auth/login.go")
	restored, err := verify.WorkingTreeSubject(context.Background(), wt)
	if err != nil || restored != fullSubject {
		t.Fatalf("restored sparse subject = %q (err=%v), want %q", restored, err, fullSubject)
	}

	// Attack 2: Materialize a modified out-of-cone file on disk.
	dirtyOut := filepath.Join(wt, "services/billing/invoice.go")
	if err := os.MkdirAll(filepath.Dir(dirtyOut), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(dirtyOut, []byte("package billing\nfunc Invoice() { panic(1) }\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	dirtySubject, err := verify.WorkingTreeSubject(context.Background(), wt)
	if err != nil {
		t.Fatalf("WorkingTreeSubject with dirty out-of-cone file: %v", err)
	}
	if dirtySubject == fullSubject {
		t.Fatal("dirty materialized out-of-cone file did not move WorkingTreeSubject")
	}

	// Remove dirty out-of-cone file and drop an untracked out-of-cone file instead.
	if err := os.Remove(dirtyOut); err != nil {
		t.Fatal(err)
	}
	untrackedOut := filepath.Join(wt, "services/billing/sneaky.go")
	if err := os.WriteFile(untrackedOut, []byte("package billing\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	untrackedSubject, err := verify.WorkingTreeSubject(context.Background(), wt)
	if err != nil {
		t.Fatalf("WorkingTreeSubject with untracked out-of-cone file: %v", err)
	}
	if untrackedSubject == fullSubject {
		t.Fatal("untracked out-of-cone file did not move WorkingTreeSubject")
	}
}

func TestSparseNonConePatternInSparseCheckoutFileIsRefused(t *testing.T) {
	r := newSubjectRepo(t)
	r.run("sparse-checkout", "init", "--cone")
	scPath := r.run("rev-parse", "--git-path", "info/sparse-checkout")
	if !filepath.IsAbs(scPath) {
		scPath = filepath.Join(r.root, scPath)
	}
	if err := os.WriteFile(scPath, []byte("/*\n!/*/\n*.go\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if s, err := r.subjectOrErr(); err == nil {
		t.Fatalf("expected non-cone wildcard pattern in info/sparse-checkout to be refused, got %s", s)
	}
}

func TestDanglingSymlinkEscapesAndPreExistingIgnoredPathsAreRefused(t *testing.T) {
	cases := []struct {
		name  string
		setup func(t *testing.T, r *subjectRepo)
	}{
		{
			name: "dangling relative escape outside repo",
			setup: func(t *testing.T, r *subjectRepo) {
				if err := os.Symlink("../../outside-nonexistent-file", filepath.Join(r.root, "escape")); err != nil {
					t.Fatal(err)
				}
			},
		},
		{
			name: "dangling absolute escape outside repo",
			setup: func(t *testing.T, r *subjectRepo) {
				missing := filepath.Join(t.TempDir(), "missing-outside.txt")
				if err := os.Symlink(missing, filepath.Join(r.root, "abs_escape")); err != nil {
					t.Fatal(err)
				}
			},
		},
		{
			name: "dangling symlink into .git",
			setup: func(t *testing.T, r *subjectRepo) {
				if err := os.Symlink(".git/hooks/missing-hook", filepath.Join(r.root, "git_link")); err != nil {
					t.Fatal(err)
				}
			},
		},
		{
			name: "dangling symlink into tool state",
			setup: func(t *testing.T, r *subjectRepo) {
				if err := os.Symlink(".devcouncil/missing.json", filepath.Join(r.root, "tool_link")); err != nil {
					t.Fatal(err)
				}
			},
		},
		{
			name: "dangling symlink into ignored build/ before build/ exists on disk",
			setup: func(t *testing.T, r *subjectRepo) {
				if err := os.Symlink("build/future_generated.go", filepath.Join(r.root, "impl.go")); err != nil {
					t.Fatal(err)
				}
			},
		},
		{
			name: "chained staged symlink escaping repo",
			setup: func(t *testing.T, r *subjectRepo) {
				if err := os.MkdirAll(filepath.Join(r.root, "sub"), 0o755); err != nil {
					t.Fatal(err)
				}
				if err := os.Symlink("../../outside-missing", filepath.Join(r.root, "sub/hop")); err != nil {
					t.Fatal(err)
				}
				if err := os.Symlink("sub/hop", filepath.Join(r.root, "entry")); err != nil {
					t.Fatal(err)
				}
			},
		},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			r := newSubjectRepo(t)
			tc.setup(t, r)
			if s, err := r.subjectOrErr(); err == nil {
				t.Fatalf("%s was accepted with subject %s; want refusal", tc.name, s)
			}
		})
	}
}
