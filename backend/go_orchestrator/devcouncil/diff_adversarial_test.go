package devcouncil_test

import (
	"os"
	"path/filepath"
	"runtime"
	"sort"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil"
)

func taskDiffArgs(staged bool, planned []dc.PlannedFile, paths ...string) devcouncil.GetDiffArgs {
	return devcouncil.GetDiffArgs{TaskID: "T", TaskFound: true, DBInitialized: true, Staged: staged, PlannedEntries: planned, Paths: paths}
}

func diffPaths(d devcouncil.DiffResult) []string {
	var out []string
	for _, f := range d.Files {
		out = append(out, f.Path)
	}
	sort.Strings(out)
	return out
}

var starPlan = []dc.PlannedFile{{Path: "*", AllowedChange: dc.ChangeModify}}

// Pinned: names that look like pathspec magic or git options are one file each,
// never an option and never a second pathspec, in both modes.
func TestGetDiffTaskScopedMagicAndOptionNames(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("these names are not valid on Windows")
	}
	root := newRepo(t)
	names := []string{":(top)x.txt", ":!src", ":(exclude)src/a.py", "-p", "--output=pwned", "[ab].txt", "*.py", "sp ace.txt"}
	for _, n := range names {
		write(t, root, n, "content of "+n+"\n")
	}
	write(t, root, "src/a.py", "VALUE = 2\n")
	whole := mustDiff(t, getDiff(t, root, taskDiffArgs(false, starPlan)))
	if !whole.OK {
		t.Fatalf("whole: %+v", whole)
	}
	got := map[string]bool{}
	for _, p := range diffPaths(whole) {
		got[p] = true
	}
	for _, n := range append([]string{"src/a.py"}, names...) {
		if !got[n] {
			t.Errorf("no-paths mode lost %q: %v", n, diffPaths(whole))
		}
	}
	if _, err := os.Stat(filepath.Join(root, "pwned")); err == nil {
		t.Error("--output=pwned was honoured as a git option")
	}
	for _, n := range names {
		one := mustDiff(t, getDiff(t, root, taskDiffArgs(false, starPlan, n)))
		if !one.OK || len(one.Files) != 1 || one.Files[0].Path != n {
			t.Errorf("explicit %q: ok=%v files=%v err=%q", n, one.OK, diffPaths(one), one.Error)
		}
	}
}

// Pinned: a changed path that is a symlink out of the repository is refused or
// left out; the target's content never reaches the diff, in both modes.
func TestGetDiffTaskScopedSymlinkToOutside(t *testing.T) {
	root := newRepo(t)
	outside := t.TempDir()
	write(t, outside, "o.txt", "synthetic external sentinel\n")
	if err := os.Symlink(filepath.Join(outside, "o.txt"), filepath.Join(root, "link.txt")); err != nil {
		t.Skipf("symlinks unavailable: %v", err)
	}
	for _, paths := range [][]string{nil, {"link.txt"}} {
		d := mustDiff(t, getDiff(t, root, taskDiffArgs(false, starPlan, paths...)))
		assertAbsent(t, d, "synthetic external sentinel")
	}
}

// Pinned: a deleted planned file is listed with status D in both modes.
func TestGetDiffTaskScopedDeletedFile(t *testing.T) {
	root := newRepo(t)
	if err := os.Remove(filepath.Join(root, "src", "a.py")); err != nil {
		t.Fatal(err)
	}
	for _, paths := range [][]string{nil, {"src/a.py"}} {
		d := mustDiff(t, getDiff(t, root, taskDiffArgs(false, starPlan, paths...)))
		if len(d.Files) != 1 || d.Files[0].Path != "src/a.py" || d.Files[0].Status != "D" {
			t.Errorf("paths=%v: %+v", paths, d.Files)
		}
	}
}
