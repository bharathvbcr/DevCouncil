package verify_test

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// committedRepo returns a repository with one commit, so HEAD exists.
func committedRepo(t *testing.T) string {
	t.Helper()
	root := t.TempDir()
	runGitIn(t, root, "init", "-q")
	if err := os.WriteFile(filepath.Join(root, "a.txt"), []byte("a\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	runGitIn(t, root, "add", "a.txt")
	runGitIn(t, root, "commit", "-q", "-m", "one")
	return root
}

// failingGit puts a git on PATH that answers `version` and `diff HEAD` the
// way a broken repository would, answers `rev-parse --verify` with revParse
// (a shell fragment), and runs the real git for everything else.
func failingGit(t *testing.T, revParse string) {
	t.Helper()
	real := testsupport.Tool(t, "git")
	script := "#!/bin/sh\n" +
		"for a in \"$@\"; do\n" +
		"  case \"$a\" in\n" +
		"    version|--version) exec '" + real + "' \"$@\" ;;\n" +
		"  esac\n" +
		"done\n" +
		"case \" $* \" in\n" +
		"  *\" rev-parse --verify \"*) " + revParse + " ;;\n" +
		"  *\" diff \"*\" HEAD \"*) echo 'fatal: bad object HEAD' >&2; exit 128 ;;\n" +
		"esac\n" +
		"exec '" + real + "' \"$@\"\n"
	bin := t.TempDir()
	// #nosec G306 -- an executable stand-in; the execute bit is the point.
	if err := os.WriteFile(filepath.Join(bin, "git"), []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", bin+string(os.PathListSeparator)+os.Getenv("PATH"))
}

// A git that fails on HEAD for any reason but "this repository has no commits"
// must not be read as an empty repository: that would clear the change list,
// skip the diff and let a gate pass on nothing.
func TestAFailedHeadProbeInANonEmptyRepoIsNotAnEmptyRepo(t *testing.T) {
	cases := map[string]string{
		"lock error":                 "echo 'fatal: Unable to create index.lock: File exists' >&2; exit 128",
		"corrupt repository":         "echo 'fatal: not a git repository' >&2; exit 128",
		"silent exit 1, HEAD exists": "exit 1",
	}
	for name, revParse := range cases {
		t.Run(name, func(t *testing.T) {
			root := committedRepo(t)
			if err := os.WriteFile(filepath.Join(root, "a.txt"), []byte("changed\n"), 0o644); err != nil {
				t.Fatal(err)
			}
			failingGit(t, revParse)
			state, err := verify.CollectGitStateReport(context.Background(), root)
			if err == nil {
				t.Fatalf("collected state with changed=%v empty=%v from a git that could not read HEAD", state.Changed, state.Empty)
			}
			if _, err := verify.WorkingTreeSubject(context.Background(), root); err == nil {
				t.Fatal("WorkingTreeSubject built a subject from a git that could not read HEAD")
			}
		})
	}
}

// The one answer that is "no commits": an unborn branch. It still lists the
// untracked files.
func TestAnUnbornBranchIsStillAnEmptyRepo(t *testing.T) {
	root := t.TempDir()
	runGitIn(t, root, "init", "-q")
	if err := os.WriteFile(filepath.Join(root, "new.txt"), []byte("x\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	state, err := verify.CollectGitStateReport(context.Background(), root)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(strings.Join(state.Changed, ","), "new.txt") {
		t.Fatalf("changed=%v, want new.txt", state.Changed)
	}
}
