package verify

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestFilePtrOmitsEmpty(t *testing.T) {
	if filePtr("") != nil {
		t.Fatal("empty file must be JSON null")
	}
	p := filePtr("src/a.go")
	if p == nil || *p != "src/a.go" {
		t.Fatal("non-empty file must be kept")
	}
}

func TestWriteBlockedCorrectionWritesOnBlock(t *testing.T) {
	root := t.TempDir()
	result := MCPResult{
		Passed: false,
		NextActions: []NextAction{
			{Action: "Revert changes to extra.go or append it."},
		},
	}
	gaps := []Gap{
		{
			ID: "GAP-T1-ORPHAN-1", GapType: "orphan_diff", Severity: "high",
			Blocking: true, RecommendedFix: "revert extra.go", File: filePtr("extra.go"),
		},
		{
			ID: "GAP-T1-FILE-1", GapType: "planned_file_not_changed",
			Blocking: false, RecommendedFix: "modify src/a.go",
		},
	}
	writeBlockedCorrection(root, "T1", &result, gaps)
	if result.CorrectionPath == "" {
		t.Fatal("blocked verify must write a repair brief")
	}
	if !strings.HasPrefix(result.CorrectionPath, filepath.Join(root, ".devcouncil", "corrections")) {
		t.Fatalf("path=%q", result.CorrectionPath)
	}
	raw, err := os.ReadFile(result.CorrectionPath)
	if err != nil {
		t.Fatal(err)
	}
	var got map[string]any
	if err := json.Unmarshal(raw, &got); err != nil {
		t.Fatal(err)
	}
	gapsJSON, _ := got["gaps"].([]any)
	if len(gapsJSON) != 1 {
		t.Fatalf("repair brief must carry only blocking gaps, got %v", got["gaps"])
	}
}

func TestWriteBlockedCorrectionSkipsPass(t *testing.T) {
	root := t.TempDir()
	result := MCPResult{Passed: true}
	writeBlockedCorrection(root, "T1", &result, []Gap{{Blocking: true}})
	if result.CorrectionPath != "" {
		t.Fatal("passed verify must not write a repair brief")
	}
	if entries, err := os.ReadDir(filepath.Join(root, ".devcouncil", "corrections")); !os.IsNotExist(err) && len(entries) != 0 {
		t.Fatalf("unexpected corrections: %v %v", entries, err)
	}
}

func TestWriteBlockedCorrectionSkipsUnmeasured(t *testing.T) {
	root := t.TempDir()
	result := MCPResult{Passed: false, VerificationSkipped: true}
	writeBlockedCorrection(root, "T1", &result, []Gap{{Blocking: true}})
	if result.CorrectionPath != "" {
		t.Fatal("skipped verification must not write a repair brief")
	}
}

func TestCorrectionPathOmittedWhenEmpty(t *testing.T) {
	raw, err := json.Marshal(MCPResult{OK: true, TaskID: "T"})
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(string(raw), "correction") {
		t.Fatalf("empty correction_path must omit: %s", raw)
	}
}
