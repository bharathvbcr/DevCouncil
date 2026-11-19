package devcouncil_test

import (
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
)

var renamePlan = []dc.PlannedFile{
	{Path: "src/*", AllowedChange: dc.ChangeModify},
	{Path: "ro_*", AllowedChange: dc.ChangeReadOnly},
}

func bothModes(t *testing.T, root string, plan []dc.PlannedFile, explicit ...string) (whole, listed []string) {
	t.Helper()
	w := mustDiff(t, getDiff(t, root, taskDiffArgs(true, plan)))
	e := mustDiff(t, getDiff(t, root, taskDiffArgs(true, plan, explicit...)))
	return diffPaths(w), diffPaths(e)
}

// A staged rename out of a planned file into a read_only directory. git collapses
// the pair to its new name, which is read_only, so the no-paths listing never
// offered the editable old name and the deletion of a planned file vanished from
// get_diff, while an explicit request showed it.
func TestGetDiffRenameAcrossReadOnlyBoundary(t *testing.T) {
	root := newRepo(t)
	runGit(t, root, "mv", "src/a.py", "ro_a.py")
	whole, explicit := bothModes(t, root, renamePlan, "src/a.py")
	if len(explicit) != 1 {
		t.Fatalf("explicit request: %v", explicit)
	}
	if strings.Join(whole, ",") != strings.Join(explicit, ",") {
		t.Errorf("staged rename src/a.py -> ro_a.py: explicit paths show %v, no-paths shows %v", explicit, whole)
	}
}

// The other direction: a read_only source moved into the plan. Only the planned
// destination is in scope, in both modes.
func TestGetDiffRenameIntoThePlanFromReadOnly(t *testing.T) {
	root := newRepo(t)
	runGit(t, root, "mv", "src/a.py", "ro_a.py")
	runGit(t, root, "commit", "-qm", "move out")
	runGit(t, root, "mv", "ro_a.py", "src/b.py")
	whole, explicit := bothModes(t, root, renamePlan, "src/b.py")
	if len(explicit) != 1 || explicit[0] != "src/b.py" {
		t.Fatalf("explicit request: %v", explicit)
	}
	if strings.Join(whole, ",") != strings.Join(explicit, ",") {
		t.Errorf("staged rename ro_a.py -> src/b.py: explicit paths show %v, no-paths shows %v", explicit, whole)
	}
}

// A staged rename from a planned file to an unplanned directory: the deletion of
// the planned name is the whole in-scope diff, in both modes.
func TestGetDiffRenameOutOfThePlan(t *testing.T) {
	root := newRepo(t)
	write(t, root, "docs/keep.txt", "k\n")
	runGit(t, root, "mv", "src/a.py", "docs/x.py")
	whole, explicit := bothModes(t, root, renamePlan, "src/a.py")
	if strings.Join(whole, ",") != strings.Join(explicit, ",") || len(whole) != 1 || whole[0] != "src/a.py" {
		t.Errorf("staged rename src/a.py -> docs/x.py: explicit %v, no-paths %v, want [src/a.py] in both", explicit, whole)
	}
}
