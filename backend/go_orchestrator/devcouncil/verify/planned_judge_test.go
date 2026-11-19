package verify_test

import (
	"context"
	"fmt"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/policy"
)

// The three scope detectors read a plan entry with the one planned-path judge,
// so a glob, "./", backslash or case-differing spelling means what it means to
// the write gate. Each row names the spelling, the change, and whether the
// plan covers it. Matching is case-sensitive, like the gate's planned-file
// rung, so a case variant is not a match on either surface.
func TestDetectorsReadPlanEntriesWithThePlannedPathJudge(t *testing.T) {
	rows := []struct {
		name    string
		planned string
		changed string
		covered bool
	}{
		{"exact", "src/a.go", "src/a.go", true},
		{"glob", "src/*.go", "src/a.go", true},
		{"glob crosses a separator", "src/*.go", "src/sub/a.go", true},
		{"dot-slash plan entry", "./src/a.go", "src/a.go", true},
		{"dot-slash changed path", "src/a.go", "./src/a.go", true},
		{"backslash plan entry", "src\\a.go", "src/a.go", true},
		{"dot-slash glob", "./src/*.go", "src/a.go", true},
		{"glob that does not match", "src/*.go", "docs/a.md", false},
		{"case variant is not the entry", "SRC/a.go", "src/a.go", false},
		{"sibling of an exact entry", "src/a.go", "src/b.go", false},
	}
	for _, row := range rows {
		t.Run(row.name, func(t *testing.T) {
			planned := []dc.PlannedFile{{Path: row.planned, AllowedChange: dc.ChangeModify}}
			changed := []string{row.changed}
			orphan := verify.DetectOrphanDiffGaps("T", planned, changed, nil)
			notChanged := verify.DetectPlannedFileGaps("T", planned, changed)
			if row.covered {
				if len(orphan) != 0 {
					t.Errorf("orphan_diff for a change the plan covers: %+v", orphan)
				}
				if len(notChanged) != 0 {
					t.Errorf("planned_file_not_changed for an entry the change satisfies: %+v", notChanged)
				}
				return
			}
			if len(orphan) != 1 || !orphan[0].Blocking || orphan[0].GapType != "orphan_diff" {
				t.Errorf("want one blocking orphan_diff, got %+v", orphan)
			}
			if len(notChanged) != 1 {
				t.Errorf("want planned_file_not_changed for an entry nothing matched, got %+v", notChanged)
			}
		})
	}
}

func TestDependencyRiskReadsPlanEntriesWithThePlannedPathJudge(t *testing.T) {
	for _, entry := range []string{"package.json", "./package.json", "*.json", "web\\package.json"} {
		changed := "package.json"
		if entry == "web\\package.json" {
			changed = "web/package.json"
		}
		planned := []dc.PlannedFile{{Path: entry, AllowedChange: dc.ChangeModify}}
		if gaps := verify.DetectDependencyRiskGaps("T", planned, []string{changed}); len(gaps) != 0 {
			t.Errorf("plan entry %q covers %s, got %+v", entry, changed, gaps)
		}
	}
	planned := []dc.PlannedFile{{Path: "./package.json", AllowedChange: dc.ChangeReadOnly}}
	if gaps := verify.DetectDependencyRiskGaps("T", planned, []string{"package.json"}); len(gaps) != 1 {
		t.Errorf("a read_only entry must not authorise a manifest edit, got %+v", gaps)
	}
}

// A read_only entry stays read-only in every spelling, and it wins over an
// editable entry that matches the same path whichever comes first.
func TestReadOnlyEntriesStayReadOnlyInEverySpelling(t *testing.T) {
	for _, entry := range []string{"src/a.go", "./src/a.go", "src\\a.go", "src/*.go"} {
		for _, order := range []string{"read-only first", "editable first"} {
			ro := dc.PlannedFile{Path: entry, AllowedChange: dc.ChangeReadOnly}
			rw := dc.PlannedFile{Path: "src/a.go", AllowedChange: dc.ChangeModify}
			planned := []dc.PlannedFile{ro, rw}
			if order == "editable first" {
				planned = []dc.PlannedFile{rw, ro}
			}
			gaps := verify.DetectOrphanDiffGaps("T", planned, []string{"src/a.go"}, nil)
			if len(gaps) != 1 || gaps[0].GapType != "oracle_tampering" || !gaps[0].Blocking {
				t.Errorf("%q (%s): want one blocking oracle_tampering, got %+v", entry, order, gaps)
			}
		}
	}
}

// Through verify.Run: an unplanned *modified* test file is a blocking orphan,
// and only an unplanned *added* one is advisory.
func TestRunKeepsAModifiedUnplannedTestFileBlocking(t *testing.T) {
	run := func(added map[string]struct{}) []verify.Gap {
		var in verify.Input
		in.Task = &store.Task{ID: "T-MOD", PlannedFiles: []dc.PlannedFile{{Path: "src/lib.py", AllowedChange: dc.ChangeModify}}}
		in.GateMode = "enforce"
		in.AllowNoRigor = true
		in.ChangedFiles = []string{"src/lib.py", "tests/test_lib.py"}
		in.AddedFiles = added
		in.DiffContent = "diff --git a/src/lib.py b/src/lib.py\n"
		gaps, _ := verify.Run(context.Background(), in)
		return gaps
	}
	orphan := func(gaps []verify.Gap) *verify.Gap {
		for i := range gaps {
			if gaps[i].GapType == "orphan_diff" && gaps[i].File != nil && *gaps[i].File == "tests/test_lib.py" {
				return &gaps[i]
			}
		}
		return nil
	}
	if g := orphan(run(nil)); g == nil || !g.Blocking {
		t.Errorf("modified unplanned test file: want a blocking orphan_diff, got %+v", g)
	}
	if g := orphan(run(map[string]struct{}{"tests/test_lib.py": {}})); g == nil || g.Blocking {
		t.Errorf("added unplanned test file: want an advisory orphan_diff, got %+v", g)
	}
}

// ft-84b49733 item 6: the scope detectors judge each changed path once, however
// many plan entries there are. Asked entry by entry, 100 entries over 300 paths
// ran the gate's pre-task rungs 30,000 times (~230 ms); the bound is one
// evaluation per changed path per detector call, counted rather than timed.
func TestScopeDetectorsEvaluateHardDeniedOncePerChangedPath(t *testing.T) {
	root := t.TempDir()
	var planned []dc.PlannedFile
	for i := 0; i < 100; i++ {
		planned = append(planned, dc.PlannedFile{Path: fmt.Sprintf("pkg%d/*.go", i), AllowedChange: dc.ChangeModify})
	}
	var changed []string
	for i := 0; i < 300; i++ {
		// Outside every glob, so no entry ends the search early.
		changed = append(changed, fmt.Sprintf("other%d/f.go", i), fmt.Sprintf("other%d/go.mod", i))
	}
	for name, detect := range map[string]func(){
		"planned_file_not_changed": func() { verify.DetectPlannedFileGapsIn(root, "T", planned, changed) },
		"orphan_diff":              func() { verify.DetectOrphanDiffGapsIn(root, "T", planned, changed, nil) },
		"dependency_risk":          func() { verify.DetectDependencyRiskGapsIn(root, "T", planned, changed) },
	} {
		before := policy.HardDeniedEvaluations()
		detect()
		if got := policy.HardDeniedEvaluations() - before; got > int64(len(changed)) {
			t.Errorf("%s: %d HardDenied evaluations for %d planned entries and %d changed paths, want at most %d (one per changed path)",
				name, got, len(planned), len(changed), len(changed))
		}
	}
}
