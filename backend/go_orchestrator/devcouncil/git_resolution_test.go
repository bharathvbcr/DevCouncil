package devcouncil_test

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
)

// A git the repository carries does not get to say what the repository
// contains. One that an absolute PATH entry places inside the checkout (as
// direnv's PATH_add does) is refused, and the refusal reads as "could not run
// git", not as "this is not a repository".
func TestRepositoryLocalGitIsNotRun(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("the planted git is a shell script")
	}
	root := t.TempDir()
	marker := filepath.Join(t.TempDir(), "ran")
	if err := os.MkdirAll(filepath.Join(root, "bin"), 0o755); err != nil {
		t.Fatal(err)
	}
	script := "#!/bin/sh\n: > \"$PLANTED_GIT_MARKER\"\necho true\n"
	// #nosec G306 -- an executable stand-in; the execute bit is the point.
	if err := os.WriteFile(filepath.Join(root, "bin", "git"), []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PLANTED_GIT_MARKER", marker)
	t.Setenv("PATH", filepath.Join(root, "bin"))

	state := devcouncil.ProbeGitRepoState(context.Background(), root)
	if _, err := os.Stat(marker); err == nil {
		t.Fatal("the repository's git ran")
	}
	if state.InRepo != nil {
		t.Fatalf("a refused git produced a repository answer: %+v", state)
	}
	if !strings.Contains(state.Reason, "inside the repository") {
		t.Fatalf("reason does not name the refusal: %q", state.Reason)
	}
}

// A root that does not exist contains no program, so git is still run and
// says what is wrong: this is not a repository. Resolution must not turn
// that into "could not run git".
func TestMissingRootIsNotARepositoryRatherThanNoGit(t *testing.T) {
	missing := filepath.Join(t.TempDir(), "no-such-repo")

	state := devcouncil.ProbeGitRepoState(context.Background(), missing)
	if state.InRepo == nil || *state.InRepo {
		t.Fatalf("a missing root read as %+v, want InRepo=false", state)
	}
}

// plantConfigPrograms points every program-bearing git setting a read can
// reach at one marker script, the way a .git/config that arrived with the
// repository could. The script prints nothing and exits 0: as an fsmonitor
// that also says "nothing changed", and as an external diff it says "no
// difference", so the planted config could hide a change as well as run.
func plantConfigPrograms(t *testing.T, root string) (marker string) {
	t.Helper()
	if runtime.GOOS == "windows" {
		t.Skip("the planted programs are shell scripts")
	}
	marker = filepath.Join(t.TempDir(), "ran")
	script := filepath.Join(t.TempDir(), "planted.sh")
	// #nosec G306 -- an executable stand-in; the execute bit is the point.
	if err := os.WriteFile(script, []byte("#!/bin/sh\necho \"$*\" >> \""+marker+"\"\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	runGit(t, root, "config", "core.fsmonitor", script)
	runGit(t, root, "config", "diff.external", script)
	runGit(t, root, "config", "diff.planted.textconv", script)
	runGit(t, root, "config", "log.showSignature", "true")
	runGit(t, root, "config", "gpg.program", script)
	runGit(t, root, "config", "filter.planted.clean", script)
	runGit(t, root, "config", "filter.planted.required", "true")
	write(t, root, ".git/info/attributes", "* diff=planted filter=planted\n")
	return marker
}

// The repository's git config does not choose what DevCouncil's own reads
// run, and does not get to hide a change from them.
func TestRepositoryGitConfigProgramsDoNotRunDuringReads(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip(err)
	}
	root := newRepo(t)
	marker := plantConfigPrograms(t, root)
	write(t, root, "src/a.py", "VALUE = 2\n")

	got, err := devcouncil.GetDiff(context.Background(), root, devcouncil.GetDiffArgs{})
	if err != nil {
		t.Fatal(err)
	}
	dr := mustDiff(t, got)
	changed, diff, empty, err := verify.CollectGitState(context.Background(), root)
	if ran, readErr := os.ReadFile(marker); readErr == nil {
		t.Fatalf("a program named in .git/config ran during reads:\n%s", ran)
	}
	if !dr.OK || len(dr.Files) != 1 || dr.Files[0].Path != "src/a.py" || !strings.Contains(dr.UnifiedDiff, "VALUE = 2") {
		t.Fatalf("get_diff lost the change: %+v", dr)
	}
	if err != nil || empty || len(changed) != 1 || !strings.Contains(diff, "VALUE = 2") {
		t.Fatalf("verification lost the change: changed=%v empty=%v err=%v diff=%q", changed, empty, err, diff)
	}
}

// markerScript writes a script that records each run and exits with status.
// A filter (status 0) passes stdin through; anything else leaves stdin
// unread, since an ssh transport holds it open and a reader would wait.
func markerScript(t *testing.T, status string) (script, marker string) {
	t.Helper()
	if runtime.GOOS == "windows" {
		t.Skip("the planted programs are shell scripts")
	}
	dir := t.TempDir()
	marker = filepath.Join(dir, "ran")
	script = filepath.Join(dir, "planted.sh")
	pass := ""
	if status == "0" {
		pass = "cat\n"
	}
	body := "#!/bin/sh\necho \"$0 $*\" >> \"" + marker + "\"\n" + pass + "exit " + status + "\n"
	// #nosec G306 -- an executable stand-in; the execute bit is the point.
	if err := os.WriteFile(script, []byte(body), 0o755); err != nil {
		t.Fatal(err)
	}
	return script, marker
}

// A submodule's own filter driver does not run during get_diff or verify's
// git state, and the dirty submodule is still reported by both.
func TestASubmoduleFilterDoesNotRunDuringGetDiffOrVerify(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip(err)
	}
	t.Setenv("GIT_CONFIG_NOSYSTEM", "1")
	t.Setenv("GIT_CONFIG_GLOBAL", os.DevNull)
	root := newRepo(t)
	src := newRepo(t)
	runGit(t, root, "-c", "protocol.file.allow=always", "submodule", "add", "-q", src, "sub")
	runGit(t, root, "commit", "-qm", "sub")
	sub := filepath.Join(root, "sub")
	script, marker := markerScript(t, "0")
	runGit(t, sub, "config", "filter.planted.clean", script)
	runGit(t, sub, "config", "filter.planted.required", "true")
	write(t, filepath.Join(root, ".git", "modules", "sub"), "info/attributes", "* filter=planted\n")
	// Same size as the committed content, so git must hash it to compare.
	write(t, sub, "src/a.py", "VALUE = 2\n")

	got, err := devcouncil.GetDiff(context.Background(), root, devcouncil.GetDiffArgs{})
	if err != nil {
		t.Fatal(err)
	}
	dr := mustDiff(t, got)
	changed, _, empty, err := verify.CollectGitState(context.Background(), root)
	if ran, readErr := os.ReadFile(marker); readErr == nil {
		t.Fatalf("the submodule's filter ran during reads:\n%s", ran)
	}
	if !dr.OK || len(dr.Files) != 1 || dr.Files[0] != (devcouncil.DiffFile{Path: "sub", Status: "M"}) {
		t.Fatalf("get_diff did not report the dirty submodule: %+v", dr)
	}
	if err != nil || empty || len(changed) != 1 || changed[0] != "sub" {
		t.Fatalf("verification did not report the dirty submodule: changed=%v empty=%v err=%v", changed, empty, err)
	}
}

// In a partial clone a read whose objects are missing fails loudly: git does
// not fetch them — the fetch runs what the repository's config names — and
// the failure is not read as an empty diff.
func TestAPartialCloneReadFailsLoudlyRatherThanFetching(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip(err)
	}
	t.Setenv("GIT_CONFIG_NOSYSTEM", "1")
	t.Setenv("GIT_CONFIG_GLOBAL", os.DevNull)
	src := newRepo(t)
	runGit(t, src, "config", "uploadpack.allowFilter", "true")
	root := filepath.Join(t.TempDir(), "clone")
	runGit(t, filepath.Dir(root), "clone", "-q", "--no-checkout", "--filter=blob:none", "file://"+src, root)
	runGit(t, root, "read-tree", "HEAD")
	script, marker := markerScript(t, "1")
	runGit(t, root, "config", "remote.origin.url", "ssh://example.invalid/repo.git")
	runGit(t, root, "config", "core.sshCommand", script)
	runGit(t, root, "config", "credential.helper", script)

	changed, diff, empty, err := verify.CollectGitState(context.Background(), root)
	if ran, readErr := os.ReadFile(marker); readErr == nil {
		t.Fatalf("verification fetched from the promisor remote and ran:\n%s", ran)
	}
	if err == nil {
		t.Fatalf("a read missing its objects answered: changed=%v empty=%v diff=%q", changed, empty, diff)
	}
	got, getErr := devcouncil.GetDiff(context.Background(), root, devcouncil.GetDiffArgs{})
	if ran, readErr := os.ReadFile(marker); readErr == nil {
		t.Fatalf("get_diff fetched from the promisor remote and ran:\n%s", ran)
	}
	if getErr == nil {
		if dr := mustDiff(t, got); dr.OK {
			t.Fatalf("get_diff answered without the objects it needed: %+v", dr)
		}
	}
}
