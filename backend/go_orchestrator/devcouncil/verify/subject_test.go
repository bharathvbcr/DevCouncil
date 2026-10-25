package verify_test

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

type subjectRepo struct {
	t    *testing.T
	root string
	git  string
}

func newSubjectRepo(t *testing.T) *subjectRepo {
	t.Helper()
	r := &subjectRepo{t: t, root: t.TempDir(), git: testsupport.Tool(t, "git")}
	r.run("init", "-q", r.root)
	r.write(".gitignore", "build/\n")
	r.write("a.go", "package a\n")
	r.run("add", ".")
	r.run("commit", "-qm", "base")
	return r
}

func (r *subjectRepo) run(args ...string) string {
	r.t.Helper()
	cmd := exec.Command(r.git, args...)
	cmd.Dir = r.root
	cmd.Env = append(os.Environ(), "GIT_AUTHOR_NAME=t", "GIT_AUTHOR_EMAIL=t@e.invalid",
		"GIT_COMMITTER_NAME=t", "GIT_COMMITTER_EMAIL=t@e.invalid")
	out, err := cmd.CombinedOutput()
	if err != nil {
		r.t.Fatalf("git %v: %v %s", args, err, out)
	}
	return strings.TrimSpace(string(out))
}

func (r *subjectRepo) write(rel, body string) {
	r.t.Helper()
	path := filepath.Join(r.root, rel)
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		r.t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(body), 0o600); err != nil {
		r.t.Fatal(err)
	}
}

func (r *subjectRepo) subject() string {
	r.t.Helper()
	s, err := verify.WorkingTreeSubject(context.Background(), r.root)
	if err != nil {
		r.t.Fatal(err)
	}
	if !strings.HasPrefix(s, verify.SubjectPrefix) {
		r.t.Fatalf("subject %q lacks %q", s, verify.SubjectPrefix)
	}
	return s
}

// A clean tree's subject is its HEAD's tree: the subject names content, and
// the committed content is the content.
func TestACleanTreesSubjectIsItsHeadTree(t *testing.T) {
	r := newSubjectRepo(t)
	if got, want := r.subject(), verify.SubjectPrefix+r.run("rev-parse", "HEAD^{tree}"); got != want {
		t.Fatalf("subject = %s, want %s", got, want)
	}
}

// Every change verify judges moves the subject, and undoing it moves it back.
// What verify never judges — ignored files and tool state — does not.
func TestTheSubjectMovesWithEveryJudgedChangeAndOnlyThose(t *testing.T) {
	r := newSubjectRepo(t)
	base := r.subject()

	r.write("a.go", "package a\n\nfunc A() {}\n")
	edited := r.subject()
	if edited == base {
		t.Fatal("editing a tracked file left the subject unchanged")
	}
	r.write("a.go", "package a\n")
	if r.subject() != base {
		t.Fatal("restoring the file did not restore the subject")
	}

	r.write("new.go", "package a\n")
	withUntracked := r.subject()
	if withUntracked == base {
		t.Fatal("an untracked file did not count")
	}
	r.write("new.go", "package a // changed\n")
	if r.subject() == withUntracked {
		t.Fatal("an untracked file's content did not count, only its name")
	}
	if err := os.Remove(filepath.Join(r.root, "new.go")); err != nil {
		t.Fatal(err)
	}

	if err := os.Remove(filepath.Join(r.root, "a.go")); err != nil {
		t.Fatal(err)
	}
	if r.subject() == base {
		t.Fatal("deleting a tracked file left the subject unchanged")
	}
	r.write("a.go", "package a\n")

	r.write("build/out.bin", "ignored")
	r.write(".devcouncil/state.json", "{}")
	r.write(".devmap/x", "y")
	if got := r.subject(); got != base {
		t.Fatalf("ignored files or tool state moved the subject: %s != %s", got, base)
	}
}

// The same content has the same subject whether it is uncommitted or
// committed: a review made before the commit still names what is verified
// after it.
func TestCommittingReviewedContentKeepsItsSubject(t *testing.T) {
	r := newSubjectRepo(t)
	r.write("a.go", "package a\n\nfunc A() {}\n")
	r.write("b.go", "package a\n")
	before := r.subject()
	r.run("add", ".")
	r.run("commit", "-qm", "change")
	if after := r.subject(); after != before {
		t.Fatalf("committing changed the subject: %s -> %s", before, after)
	}
}

// Computing the subject leaves the person's index exactly as it was: what
// they staged stays staged, and what they did not stays unstaged.
func TestTheSubjectNeverTouchesTheRealIndex(t *testing.T) {
	r := newSubjectRepo(t)
	r.write("staged.go", "package a\n")
	r.run("add", "staged.go")
	r.write("a.go", "package a // unstaged\n")
	r.write("untracked.go", "package a\n")
	cached := r.run("diff", "--cached", "--name-only")
	status := r.run("status", "--porcelain")

	r.subject()

	if got := r.run("diff", "--cached", "--name-only"); got != cached {
		t.Errorf("staged set changed: %q -> %q", cached, got)
	}
	if got := r.run("status", "--porcelain"); got != status {
		t.Errorf("status changed: %q -> %q", status, got)
	}
}

// A repository with no commit yet still has content to name.
func TestARepositoryWithNoCommitHasASubject(t *testing.T) {
	git := testsupport.Tool(t, "git")
	root := t.TempDir()
	cmd := exec.Command(git, "init", "-q", root)
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("%v %s", err, out)
	}
	if err := os.WriteFile(filepath.Join(root, "a.go"), []byte("package a\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	s, err := verify.WorkingTreeSubject(context.Background(), root)
	if err != nil || !strings.HasPrefix(s, verify.SubjectPrefix) {
		t.Fatalf("subject = %q, %v", s, err)
	}
}

// Outside a repository there is nothing to name, and that is an error, never
// a subject that would match some other empty answer.
func TestNoRepositoryIsAnError(t *testing.T) {
	if s, err := verify.WorkingTreeSubject(context.Background(), t.TempDir()); err == nil {
		t.Fatalf("subject %q outside a repository", s)
	}
}
