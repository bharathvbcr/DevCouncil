package verify_test

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
)

// Characterization: empty planned work with a malformed "*" verification
// command must reproduce the Phase-0 golden shapes (gap ids, categories,
// coverage_skipped_reason, never pass).
func TestGoldenLeasedVerifyShape(t *testing.T) {
	task := &store.Task{
		ID:         "TASK-001",
		Difficulty: "easy",
		PlannedFiles: []dc.PlannedFile{
			{Path: "src/a.py", AllowedChange: dc.ChangeModify},
		},
		ExpectedTests: []string{"*"},
	}
	in := verify.Input{
		Task:         task,
		GateMode:     "enforce",
		Sandbox:      "local",
		Difficulty:   "easy",
		ChangedFiles: nil,
		DiffContent:  "",
		DiffEmpty:    true,
		WorkPresent:  false,
		Commands:     []string{"*"},
		RunCommand: func(command string) verify.CommandOutcome {
			return verify.CommandOutcome{
				ExitCode: -1,
				Summary:  "Failed to run command: [Errno 2] No such file or directory: '*'",
			}
		},
	}
	gaps, meta := verify.Run(in)
	result := verify.ToMCP("TASK-001", gaps, meta)

	if result.Passed {
		t.Fatal("empty work must not pass")
	}
	if result.Status != "blocked" {
		t.Fatalf("status=%q want blocked", result.Status)
	}
	if result.CoverageMeasured {
		t.Fatal("coverage must not report measured when skipped")
	}
	if result.CoverageSkippedReason != "no diff to measure" {
		t.Fatalf("coverage_skipped_reason=%q", result.CoverageSkippedReason)
	}
	if result.VerificationMode != "coarse" {
		t.Fatalf("verification_mode=%q", result.VerificationMode)
	}
	if len(result.BlockingGaps) != 1 || result.BlockingGaps[0].GapType != "task_not_implemented" {
		t.Fatalf("blocking_gaps=%+v", result.BlockingGaps)
	}
	wantID := "GAP-TASK-001-NODIFF-31d6a5192b"
	if result.BlockingGaps[0].ID != wantID {
		t.Fatalf("NODIFF id=%q want %q", result.BlockingGaps[0].ID, wantID)
	}
	if len(result.NextActions) != 1 || result.NextActions[0].Category != "plan" {
		t.Fatalf("next_actions=%+v", result.NextActions)
	}
	foundBad, foundFile := false, false
	for _, a := range result.AdvisoryActions {
		if a.GapType == "invalid_verification_command" {
			foundBad = true
			if a.GapID != "GAP-TASK-001-BADCMD-53e6757ddd" {
				t.Fatalf("BADCMD id=%q", a.GapID)
			}
			if a.Category != "fix_verification" {
				t.Fatalf("BADCMD category=%q", a.Category)
			}
		}
		if a.GapType == "planned_file_not_changed" {
			foundFile = true
			if a.GapID != "GAP-TASK-001-FILEsrcapy-a71cc01ad9" {
				t.Fatalf("FILE id=%q", a.GapID)
			}
		}
	}
	if !foundBad || !foundFile {
		t.Fatalf("advisory missing: bad=%v file=%v actions=%+v", foundBad, foundFile, result.AdvisoryActions)
	}
}

func TestGoldenCLIVerifyShape(t *testing.T) {
	task := &store.Task{
		ID:         "TASK-CLI",
		Difficulty: "easy",
		PlannedFiles: []dc.PlannedFile{
			{Path: "src/a.py", AllowedChange: dc.ChangeModify},
		},
		ExpectedTests: []string{"*"},
	}
	in := verify.Input{
		Task:        task,
		GateMode:    "enforce",
		Sandbox:     "local",
		Difficulty:  "easy",
		DiffEmpty:   true,
		WorkPresent: false,
		Commands:    []string{"*"},
		RunCommand: func(string) verify.CommandOutcome {
			return verify.CommandOutcome{
				ExitCode: -1,
				Summary:  "Failed to run command: [Errno 2] No such file or directory: '*'",
			}
		},
	}
	gaps, meta := verify.Run(in)
	entry := verify.ToCLITask("TASK-CLI", gaps, meta)
	if entry.GapCount != 3 {
		t.Fatalf("gap_count=%d want 3", entry.GapCount)
	}
	if entry.BlockingGapCount != 1 {
		t.Fatalf("blocking_gap_count=%d", entry.BlockingGapCount)
	}
	ids := map[string]bool{}
	for _, g := range entry.Gaps {
		ids[g.ID] = true
	}
	for _, want := range []string{
		"GAP-TASK-CLI-NODIFF-02b5ef802a",
		"GAP-TASK-CLI-BADCMD-bc55347fb2",
		"GAP-TASK-CLI-FILEsrcapy-30ca4a038b",
	} {
		if !ids[want] {
			t.Fatalf("missing gap id %s in %+v", want, ids)
		}
	}
	cli := verify.CLIResult{
		OK:             false,
		GateMode:       "enforce",
		ProcessedTasks: 1,
		BlockedTasks:   1,
		TotalGaps:      3,
		Tasks:          []verify.TaskCLIResult{entry},
	}
	raw, err := json.Marshal(cli)
	if err != nil {
		t.Fatal(err)
	}
	var round map[string]any
	if err := json.Unmarshal(raw, &round); err != nil {
		t.Fatal(err)
	}
	if round["ok"] != false {
		t.Fatalf("ok=%v", round["ok"])
	}
}

func TestSkippedNeverPass(t *testing.T) {
	task := &store.Task{
		ID: "TASK-SKIP",
		PlannedFiles: []dc.PlannedFile{
			{Path: "src/a.go", AllowedChange: dc.ChangeModify},
		},
	}
	// Work present so no_work does not block; coverage still skipped.
	in := verify.Input{
		Task:         task,
		GateMode:     "enforce",
		ChangedFiles: []string{"src/a.go"},
		DiffContent:  "diff --git a/src/a.go b/src/a.go\n",
		DiffEmpty:    false,
		WorkPresent:  true,
		Commands:     []string{"pytest"},
		RunCommand: func(string) verify.CommandOutcome {
			return verify.CommandOutcome{Skipped: true, Reason: "pytest not installed"}
		},
	}
	gaps, meta := verify.Run(in)
	result := verify.ToMCP("TASK-SKIP", gaps, meta)
	if meta.CoverageMeasured {
		t.Fatal("unmeasured coverage must not look measured")
	}
	if meta.CoverageSkippedReason == "" {
		t.Fatal("skipped coverage needs a reason")
	}
	found := false
	for _, g := range gaps {
		if g.GapType == "skipped_verification_command" {
			found = true
			if g.Blocking {
				t.Fatal("skipped command must not be blocking by default")
			}
		}
		if g.GapType == "test_failed" && !g.Blocking {
			t.Fatal("test_failed must be blocking")
		}
	}
	if !found {
		t.Fatalf("expected skipped_verification_command, gaps=%+v", gaps)
	}
	_ = result
}

func TestStableGapIDMatchesPython(t *testing.T) {
	cases := []struct{ task, kind, want string }{
		{"TASK-001", "NODIFF", "GAP-TASK-001-NODIFF-31d6a5192b"},
		{"TASK-001", "BADCMD-*", "GAP-TASK-001-BADCMD-53e6757ddd"},
		{"TASK-001", "FILE-src/a.py", "GAP-TASK-001-FILEsrcapy-a71cc01ad9"},
	}
	for _, c := range cases {
		if got := verify.StableGapID(c.task, c.kind); got != c.want {
			t.Fatalf("%s/%s: got %s want %s", c.task, c.kind, got, c.want)
		}
	}
}

func TestPythonListRepr(t *testing.T) {
	got := verify.PythonListRepr([]string{"src/a.py"})
	if got != "['src/a.py']" {
		t.Fatalf("got %q", got)
	}
}

func TestOrphanAndDependency(t *testing.T) {
	planned := []dc.PlannedFile{{Path: "src/a.go", AllowedChange: dc.ChangeModify}}
	changed := []string{"src/a.go", "go.mod", "extra.go"}
	gaps := verify.DetectOrphanDiffGaps("T1", planned, changed, nil)
	gaps = append(gaps, verify.DetectDependencyRiskGaps("T1", planned, changed)...)
	types := map[string]int{}
	for _, g := range gaps {
		types[g.GapType]++
		if g.GapType == "orphan_diff" && g.File != nil && *g.File == "extra.go" && !g.Blocking {
			t.Fatal("non-test orphan must block")
		}
		if g.GapType == "dependency_risk" && !g.Blocking {
			t.Fatal("dependency_risk must block")
		}
	}
	if types["orphan_diff"] < 1 || types["dependency_risk"] < 1 {
		t.Fatalf("types=%v", types)
	}
}

func TestStatusFromGapsModes(t *testing.T) {
	nodiff := verify.Gap{GapType: "task_not_implemented", Blocking: true}
	failed := verify.Gap{GapType: "test_failed", Blocking: true}

	status, passed := verify.StatusFromGaps([]verify.Gap{nodiff, failed}, "")
	if !passed || status != "verified" {
		t.Fatalf("empty mode must skip quality: status=%s passed=%v", status, passed)
	}
	status, passed = verify.StatusFromGaps([]verify.Gap{nodiff, failed}, "off")
	if !passed || status != "verified" {
		t.Fatalf("off must skip quality: status=%s passed=%v", status, passed)
	}

	status, passed = verify.StatusFromGaps([]verify.Gap{failed}, "advisory")
	if !passed || status != "verified" {
		t.Fatalf("advisory must not block demotable test_failed: status=%s passed=%v", status, passed)
	}
	status, passed = verify.StatusFromGaps([]verify.Gap{nodiff}, "advisory")
	if passed || status != "blocked" {
		t.Fatalf("advisory must still block hard-safety NODIFF: status=%s passed=%v", status, passed)
	}

	status, passed = verify.StatusFromGaps([]verify.Gap{failed}, "enforce")
	if passed || status != "blocked" {
		t.Fatalf("enforce must block test_failed: status=%s passed=%v", status, passed)
	}

	status, passed = verify.StatusFromGaps([]verify.Gap{failed}, "yolo")
	if !passed || status != "verified" {
		t.Fatalf("unknown mode must not silently enforce: status=%s passed=%v", status, passed)
	}
	status, passed = verify.StatusFromGaps([]verify.Gap{failed}, "no")
	if !passed || status != "verified" {
		t.Fatalf("alias no must skip: status=%s passed=%v", status, passed)
	}
}

func TestFixtureGoldenFilesExist(t *testing.T) {
	root := filepath.Join("..", "..", "testdata", "golden")
	for _, rel := range []string{
		"mcp/verify/leased.json",
		"cli/verify/task.json",
	} {
		p := filepath.Join(root, rel)
		if _, err := os.Stat(p); err != nil {
			t.Fatalf("missing golden %s: %v", p, err)
		}
	}
}
