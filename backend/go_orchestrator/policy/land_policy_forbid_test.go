package policy

import (
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
)

// An entry naming the root forbids everything; it must not read as void.
func TestLandNForbiddenRootEntryIsNotVoid(t *testing.T) {
	g := FileGate{Root: t.TempDir(), HardRules: true}
	for _, fb := range []string{".", "./", "src/..", "src/../"} {
		task := &dc.Task{ID: "T", PlannedFiles: []dc.PlannedFile{{Path: "**", AllowedChange: dc.ChangeModify}}, ForbiddenChanges: []string{fb}}
		if d := g.EvaluateFileChange("lib/x.go", task, dc.OpModify, false); !d.Blocked() {
			t.Errorf("forbidden %q does not forbid lib/x.go: %s", fb, d.Action)
		}
	}
}
