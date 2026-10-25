package verify_test

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

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
	isolateGitConfig(t)
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

// The subject names content, not a checkout: a separate clone of the same
// commit has the same subject, and so does the same content made by hand.
func TestTheSameContentHasTheSameSubjectInAnyCheckout(t *testing.T) {
	r := newSubjectRepo(t)
	want := r.subject()
	clone := &subjectRepo{t: t, root: filepath.Join(t.TempDir(), "clone"), git: r.git}
	r.run("clone", "-q", r.root, clone.root)
	if got := clone.subject(); got != want {
		t.Fatalf("a clone's subject = %s, want %s", got, want)
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

// subjectOrErr is the subject, or "" and the error when none can be named.
func (r *subjectRepo) subjectOrErr() (string, error) {
	r.t.Helper()
	return verify.WorkingTreeSubject(context.Background(), r.root)
}

// mustNotCollide holds the one property a subject owes a review: after a
// change verify would judge, the subject either moves or cannot be named.
// Naming the same subject is the failure — an old review would vouch for new
// code.
func (r *subjectRepo) mustNotCollide(before string, change string) {
	r.t.Helper()
	after, err := r.subjectOrErr()
	if err == nil && after == before {
		r.t.Errorf("%s: the subject stayed %s, so a review of the old content would vouch for the new", change, before)
	}
}

// What git keeps in the person's index — assume-unchanged, skip-worktree, a
// stat cache it is told to trust — is not the content, and must not decide
// the subject. (Found in the adversarial round on 48a66ac3.)
func TestIndexStateCannotHideAModifiedFile(t *testing.T) {
	for name, setup := range map[string]func(r *subjectRepo){
		"assume-unchanged": func(r *subjectRepo) { r.run("update-index", "--assume-unchanged", "a.go") },
		"skip-worktree":    func(r *subjectRepo) { r.run("update-index", "--skip-worktree", "a.go") },
		"trusted stat cache": func(r *subjectRepo) {
			r.run("config", "core.trustctime", "false")
			r.run("config", "core.checkStat", "minimal")
		},
	} {
		t.Run(name, func(t *testing.T) {
			r := newSubjectRepo(t)
			old := time.Now().Add(-time.Hour)
			path := filepath.Join(r.root, "a.go")
			if err := os.Chtimes(path, old, old); err != nil {
				t.Fatal(err)
			}
			r.run("update-index", "--refresh")
			setup(r)
			before := r.subject()
			r.write("a.go", "package b\n") // the same size as "package a\n"
			if err := os.Chtimes(path, old, old); err != nil {
				t.Fatal(err)
			}
			r.mustNotCollide(before, "a same-size edit with the old mtime")
		})
	}
}

// A directory turned into a gitlink — by a nested repository, or by nothing
// more than a planted `.git` file — has files the tree does not hold.
func TestAGitlinkTheTreeCannotSeeIntoIsRefused(t *testing.T) {
	t.Run("planted gitfile", func(t *testing.T) {
		r := newSubjectRepo(t)
		r.write("pkg/sub/x.go", "package sub\n")
		r.write("pkg/sub/.git", "gitdir: "+filepath.Join(r.root, ".git")+"\n")
		if s, err := r.subjectOrErr(); err == nil {
			t.Fatalf("a directory hidden behind a planted .git file was named %s", s)
		} else if !strings.Contains(err.Error(), "pkg/sub") {
			t.Errorf("the refusal does not name the directory: %v", err)
		}
	})
	t.Run("nested repository", func(t *testing.T) {
		r := newSubjectRepo(t)
		r.write("sub/x.go", "package sub\n")
		inner := &subjectRepo{t: t, root: filepath.Join(r.root, "sub"), git: r.git}
		inner.run("init", "-q", inner.root)
		inner.run("add", ".")
		inner.run("commit", "-qm", "inner")
		if s, err := r.subjectOrErr(); err == nil {
			t.Fatalf("a nested repository's files were named %s", s)
		}
	})
}

// A rules file that ignores itself can hide new source from the tree while
// the build still compiles it.
func TestASelfIgnoringRulesFileIsRefused(t *testing.T) {
	r := newSubjectRepo(t)
	r.write("pkg/ok.go", "package pkg\n")
	before := r.subject()
	r.write("pkg/evil.go", "package pkg\nfunc init() { panic(1) }\n")
	r.write("pkg/.gitignore", "evil.go\n.gitignore\n")
	r.mustNotCollide(before, "a new file hidden by a self-ignoring .gitignore")
	if _, err := r.subjectOrErr(); err == nil || !strings.Contains(err.Error(), "pkg/.gitignore") {
		t.Errorf("err = %v, want a refusal naming pkg/.gitignore", err)
	}
	// A rules file inside a directory already ignored hides nothing new.
	r2 := newSubjectRepo(t) // .gitignore holds build/
	r2.write("build/.gitignore", "*\n")
	if _, err := r2.subjectOrErr(); err != nil {
		t.Errorf("a rules file inside an ignored directory was refused: %v", err)
	}
}

// Rules outside the tree decide what it leaves out, so they are part of the
// subject.
func TestExcludeRulesOutsideTheTreeArePartOfTheSubject(t *testing.T) {
	r := newSubjectRepo(t)
	before := r.subject()
	r.write("evil.go", "package a\nfunc init() { panic(1) }\n")
	r.write(".git/info/exclude", "# comment\nevil.go\n")
	r.mustNotCollide(before, "a new file hidden by .git/info/exclude")
	// Comments alone are no rules: the subject carries no rules digest.
	r2 := newSubjectRepo(t)
	clean := r2.subject()
	r2.write(".git/info/exclude", "# only a comment\n\n")
	if got := r2.subject(); got != clean || strings.Contains(got, "-x") {
		t.Errorf("subject = %s, want %s with no rules digest", got, clean)
	}
}

// Round 4 on 14178f9d. The bytes on disk decide the subject, whatever git
// would store for them.
func TestGitsContentTransformsCannotHideAnEdit(t *testing.T) {
	cases := map[string]struct {
		setup  func(r *subjectRepo)
		edit   func(r *subjectRepo)
		stored string
	}{
		"crlf under text=auto": {
			setup: func(r *subjectRepo) {
				r.write(".gitattributes", "* text=auto\n")
				r.write("data.txt", "a\nb\n")
			},
			edit: func(r *subjectRepo) { r.write("data.txt", "a\r\nb\r\n") },
		},
		"crlf under core.autocrlf": {
			setup: func(r *subjectRepo) {
				r.run("config", "core.autocrlf", "true")
				r.write("data.txt", "a\nb\n")
			},
			edit: func(r *subjectRepo) { r.write("data.txt", "a\r\nb\r\n") },
		},
		"ident expansion from info/attributes": {
			setup: func(r *subjectRepo) {
				r.write("v.go", "package a\n\nconst V = \"$Id$\"\n")
			},
			edit: func(r *subjectRepo) {
				r.write(".git/info/attributes", "*.go ident\n")
				r.write("v.go", "package a\n\nconst V = \"$Id: evil $\"\n")
			},
		},
		"exec bit with core.fileMode off": {
			setup: func(r *subjectRepo) {
				r.run("config", "core.fileMode", "false")
				r.write("run.sh", "#!/bin/sh\necho hi\n")
			},
			edit: func(r *subjectRepo) {
				if err := os.Chmod(filepath.Join(r.root, "run.sh"), 0o755); err != nil {
					r.t.Fatal(err)
				}
			},
		},
	}
	for name, c := range cases {
		t.Run(name, func(t *testing.T) {
			r := newSubjectRepo(t)
			c.setup(r)
			r.run("add", ".")
			r.run("commit", "-qm", "setup")
			before := r.subject()
			c.edit(r)
			r.mustNotCollide(before, name)
		})
	}
}

// A symlink stored as a plain file (core.symlinks off) holding the same text
// is a different thing on disk, and so a different subject.
func TestASymlinkReplacedByItsTextIsADifferentSubject(t *testing.T) {
	r := newSubjectRepo(t)
	if err := os.Symlink("a.go", filepath.Join(r.root, "alias.go")); err != nil {
		t.Fatal(err)
	}
	r.run("add", ".")
	r.run("commit", "-qm", "link")
	before := r.subject()
	r.run("config", "core.symlinks", "false")
	if err := os.Remove(filepath.Join(r.root, "alias.go")); err != nil {
		t.Fatal(err)
	}
	r.write("alias.go", "a.go")
	r.mustNotCollide(before, "a symlink replaced by a file holding its target text")
}

func TestASelfIgnoringRulesFileStagedInTheRealIndexIsRefused(t *testing.T) {
	r := newSubjectRepo(t)
	r.write("pkg/ok.go", "package pkg\n")
	r.run("add", ".")
	r.run("commit", "-qm", "pkg")
	before := r.subject()
	r.write("pkg/.gitignore", "evil.go\n.gitignore\n")
	r.run("add", "-f", "pkg/.gitignore")
	r.write("pkg/evil.go", "package pkg\nfunc init() { panic(1) }\n")
	r.mustNotCollide(before, "a self-ignoring rules file staged in the person's index")
}

func TestAGitlinkDirectoryMustBeItsOwnRepository(t *testing.T) {
	setup := func(t *testing.T) (*subjectRepo, *subjectRepo) {
		r := newSubjectRepo(t)
		inner := &subjectRepo{t: t, root: filepath.Join(r.root, "sub"), git: r.git}
		inner.write("x.go", "package sub\n")
		inner.run("init", "-q", inner.root)
		inner.run("add", ".")
		inner.run("commit", "-qm", "inner")
		r.run("add", ".")
		r.run("commit", "-qm", "gitlink")
		return r, inner
	}
	t.Run("populated without a repository", func(t *testing.T) {
		r, inner := setup(t)
		before := r.subject()
		if err := os.RemoveAll(filepath.Join(inner.root, ".git")); err != nil {
			t.Fatal(err)
		}
		r.write("sub/evil.go", "package sub\n")
		r.mustNotCollide(before, "files in a gitlink directory with no repository")
	})
	t.Run("a decoy gitfile", func(t *testing.T) {
		r, inner := setup(t)
		before := r.subject()
		decoy := filepath.Join(t.TempDir(), "decoy.git")
		r.run("clone", "-q", "--bare", inner.root, decoy)
		r.run("--git-dir", decoy, "config", "core.worktree", t.TempDir())
		if err := os.RemoveAll(filepath.Join(inner.root, ".git")); err != nil {
			t.Fatal(err)
		}
		r.write("sub/.git", "gitdir: "+decoy+"\n")
		r.write("sub/x.go", "package sub\n// changed\n")
		r.mustNotCollide(before, "a gitlink whose repository's working tree is elsewhere")
	})
}

func TestASymlinkIntoGitIsRefused(t *testing.T) {
	r := newSubjectRepo(t)
	r.write(".git/payload.go.txt", "package a\n")
	if err := os.Symlink(".git/payload.go.txt", filepath.Join(r.root, "impl.go")); err != nil {
		t.Fatal(err)
	}
	if s, err := r.subjectOrErr(); err == nil {
		t.Fatalf("a symlink into .git was named %s", s)
	}
}

// Tool state is never judged, whether edited or staged.
func TestStagedToolStateDoesNotMoveTheSubject(t *testing.T) {
	r := newSubjectRepo(t)
	r.write(".devcouncil/config.yaml", "a: 1\n")
	r.run("add", "-f", ".devcouncil/config.yaml")
	r.run("commit", "-qm", "tool state")
	before := r.subject()
	r.write(".devcouncil/config.yaml", "a: 2\n")
	if s := r.subject(); s != before {
		t.Errorf("an edit to tool state moved the subject %s -> %s", before, s)
	}
	r.run("add", "-f", ".devcouncil/config.yaml")
	if s := r.subject(); s != before {
		t.Errorf("staging tool state moved the subject %s -> %s", before, s)
	}
}

// isolateGitConfig gives git (this test's and the subject's) an empty user
// and system config, so a machine's own excludes file cannot decide what a
// test expects.
func isolateGitConfig(t *testing.T) string {
	t.Helper()
	home := t.TempDir()
	global := filepath.Join(home, ".gitconfig")
	if err := os.WriteFile(global, nil, 0o600); err != nil {
		t.Fatal(err)
	}
	t.Setenv("HOME", home)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	t.Setenv("GIT_CONFIG_GLOBAL", global)
	t.Setenv("GIT_CONFIG_NOSYSTEM", "1")
	return home
}

// Round 2 on c8f63bc0: each of these changed content verify would judge and
// left the subject where it was.

func TestAnUppercaseRulesFileCannotHideSource(t *testing.T) {
	r := newSubjectRepo(t)
	before := r.subject()
	r.write("pkg/evil.go", "package pkg\nfunc init() { panic(1) }\n")
	r.write("pkg/.GITIGNORE", ".GITIGNORE\nevil.go\n")
	r.mustNotCollide(before, "a new file hidden by an uppercase .GITIGNORE")
}

func TestUntrackedFilesInASubmoduleAreSeen(t *testing.T) {
	for _, withModulesFile := range []bool{true, false} {
		r := newSubjectRepo(t)
		inner := &subjectRepo{t: t, root: filepath.Join(r.root, "sub"), git: r.git}
		inner.write("x.go", "package sub\n")
		inner.run("init", "-q", inner.root)
		inner.run("add", ".")
		inner.run("commit", "-qm", "inner")
		if withModulesFile {
			r.write(".gitmodules", "[submodule \"sub\"]\n\tpath = sub\n\turl = ./sub\n")
		}
		r.run("add", ".")
		r.run("commit", "-qm", "with a gitlink")
		before := r.subject()
		r.write("sub/evil.go", "package sub\nfunc init() { panic(1) }\n")
		r.mustNotCollide(before, "an untracked file in a submodule")
		if err := os.Remove(filepath.Join(r.root, "sub/evil.go")); err != nil {
			t.Fatal(err)
		}
		r.write("sub/x.go", "package sub\n// changed\n")
		r.mustNotCollide(before, "a tracked change in a submodule")
	}
}

func TestASymlinkWhoseTargetTheTreeLacksIsRefused(t *testing.T) {
	r := newSubjectRepo(t)
	r.write("build/impl.go.txt", "package a\n")
	if err := os.Symlink("build/impl.go.txt", filepath.Join(r.root, "impl.go")); err != nil {
		t.Fatal(err)
	}
	if s, err := r.subjectOrErr(); err == nil {
		t.Fatalf("a symlink into an ignored directory was named %s", s)
	}
	rd := newSubjectRepo(t)
	rd.write("build/impl.go", "package a\n")
	if err := os.Symlink("build", filepath.Join(rd.root, "gen")); err != nil {
		t.Fatal(err)
	}
	if s, err := rd.subjectOrErr(); err == nil {
		t.Fatalf("a symlinked directory onto an ignored directory was named %s", s)
	}
	r2 := newSubjectRepo(t)
	if err := os.Symlink(t.TempDir(), filepath.Join(r2.root, "out")); err != nil {
		t.Fatal(err)
	}
	if s, err := r2.subjectOrErr(); err == nil {
		t.Fatalf("a symlink out of the repository was named %s", s)
	}
	// A symlink to tracked content, or a dangling one, is fine: the tree
	// holds the target, or there is none.
	r3 := newSubjectRepo(t)
	for name, target := range map[string]string{"alias.go": "a.go", "dangling": "nowhere"} {
		if err := os.Symlink(target, filepath.Join(r3.root, name)); err != nil {
			t.Fatal(err)
		}
	}
	if _, err := r3.subjectOrErr(); err != nil {
		t.Errorf("a symlink to tracked content was refused: %v", err)
	}
}

func TestExcludesFromAnyScopeArePartOfTheSubject(t *testing.T) {
	t.Run("through include.path", func(t *testing.T) {
		r := newSubjectRepo(t)
		before := r.subject()
		r.write("rules", "evil.go\n")
		r.write("inc.cfg", "[core]\n\texcludesFile = "+filepath.Join(r.root, "rules")+"\n")
		r.run("config", "include.path", filepath.Join(r.root, "inc.cfg"))
		r.write("evil.go", "package a\nfunc init() { panic(1) }\n")
		r.mustNotCollide(before, "a file hidden by an excludesFile set through include.path")
	})
	t.Run("global", func(t *testing.T) {
		r := newSubjectRepo(t)
		before := r.subject()
		rules := filepath.Join(t.TempDir(), "rules")
		if err := os.WriteFile(rules, []byte("evil.go\n"), 0o600); err != nil {
			t.Fatal(err)
		}
		r.run("config", "--global", "core.excludesFile", rules)
		r.write("evil.go", "package a\nfunc init() { panic(1) }\n")
		r.mustNotCollide(before, "a file hidden by a global excludesFile")
	})
	t.Run("xdg default", func(t *testing.T) {
		r := newSubjectRepo(t)
		before := r.subject()
		r.write("evil.go", "package a\nfunc init() { panic(1) }\n")
		xdg := filepath.Join(os.Getenv("XDG_CONFIG_HOME"), "git")
		if err := os.MkdirAll(xdg, 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(xdg, "ignore"), []byte("evil.go\n"), 0o600); err != nil {
			t.Fatal(err)
		}
		r.mustNotCollide(before, "a file hidden by the XDG default excludes file")
	})
}

func TestACaseOnlyRenameMovesTheSubject(t *testing.T) {
	r := newSubjectRepo(t)
	r.write("z_test.go", "package a\n")
	r.run("add", ".")
	r.run("commit", "-qm", "test file")
	before := r.subject()
	if err := os.Rename(filepath.Join(r.root, "z_test.go"), filepath.Join(r.root, "tmpname")); err != nil {
		t.Fatal(err)
	}
	if err := os.Rename(filepath.Join(r.root, "tmpname"), filepath.Join(r.root, "z_Test.go")); err != nil {
		t.Fatal(err)
	}
	r.mustNotCollide(before, "a case-only rename")
}

func TestASparseCheckoutIsRefused(t *testing.T) {
	r := newSubjectRepo(t)
	r.run("config", "core.sparseCheckout", "true")
	if s, err := r.subjectOrErr(); err == nil {
		t.Fatalf("a sparse checkout was named %s", s)
	}
}
