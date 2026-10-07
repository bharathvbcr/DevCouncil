package devcouncil_test

import (
	"context"
	"encoding/json"
	"path/filepath"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// TestGetGapsReportsAGapsLinkageAndLocation reads gaps back through
// devcouncil_get_gaps from a real store and checks the JSON an MCP client
// receives.
//
// The handler builds each gap field by field, so a field the store carries is
// still lost here unless it is named. An acceptance_criteria_unproven gap that
// does not say which requirement and criterion it is about, or which method was
// expected, tells an agent that something is unproven but not what; a gap with
// no file, line or command tells it something is wrong but not where.
func TestGetGapsReportsAGapsLinkageAndLocation(t *testing.T) {
	ctx := context.Background()
	client := store.New(testsupport.DCStore(t), filepath.Join(t.TempDir(), "state.sqlite"))

	req, ac, method := "REQ-1", "AC-1.1", "integration_test"
	file, line, cmd := "src/a.py", 9, "pytest tests/test_a.py"
	if err := client.GapsReplace(ctx, "TASK-1", []store.GapRow{
		{
			ID: "G-AC", Severity: "high", GapType: "acceptance_criteria_unproven",
			TaskID: "TASK-1", Description: "AC-1.1 unproven", RecommendedFix: "prove it",
			Blocking: true, EvidenceJSON: []byte(`[]`),
			RequirementID: &req, AcceptanceCriterionID: &ac, ExpectedVerificationMethod: &method,
			File: &file, Line: &line, SuggestedCommand: &cmd,
		},
		{
			ID: "G-STUB", Severity: "low", GapType: "stub_detected",
			TaskID: "TASK-1", Description: "stub", RecommendedFix: "fill it",
			EvidenceJSON: []byte(`[]`),
		},
	}); err != nil {
		t.Fatalf("GapsReplace: %v", err)
	}

	out, err := devcouncil.NewRegistry(t.TempDir(), client, nil).
		Call(ctx, "devcouncil_get_gaps", map[string]any{"task_id": "TASK-1"})
	if err != nil {
		t.Fatalf("Call: %v", err)
	}
	raw, err := json.Marshal(out)
	if err != nil {
		t.Fatalf("marshal: %v", err)
	}
	var payload struct {
		OK   bool                         `json:"ok"`
		Gaps []map[string]json.RawMessage `json:"gaps"`
	}
	if err := json.Unmarshal(raw, &payload); err != nil {
		t.Fatalf("unmarshal %s: %v", raw, err)
	}
	if !payload.OK || len(payload.Gaps) != 2 {
		t.Fatalf("payload = %s, want ok with two gaps", raw)
	}

	want := map[string]map[string]string{
		"G-AC": {
			"requirement_id":               `"REQ-1"`,
			"acceptance_criterion_id":      `"AC-1.1"`,
			"expected_verification_method": `"integration_test"`,
			"file":                         `"src/a.py"`,
			"line":                         `9`,
			"suggested_command":            `"pytest tests/test_a.py"`,
		},
		// Present and null, not absent: a missing key cannot be told apart
		// from a server that does not report these fields at all.
		"G-STUB": {
			"requirement_id":               `null`,
			"acceptance_criterion_id":      `null`,
			"expected_verification_method": `null`,
			"file":                         `null`,
			"line":                         `null`,
			"suggested_command":            `null`,
		},
	}
	for _, gap := range payload.Gaps {
		var id string
		if err := json.Unmarshal(gap["id"], &id); err != nil {
			t.Fatalf("gap id in %s: %v", raw, err)
		}
		expected, known := want[id]
		if !known {
			t.Errorf("unexpected gap %q in %s", id, raw)
			continue
		}
		delete(want, id)
		for key, value := range expected {
			got, ok := gap[key]
			if !ok {
				t.Errorf("%s: %s missing from the reply", id, key)
				continue
			}
			if string(got) != value {
				t.Errorf("%s: %s = %s, want %s", id, key, got, value)
			}
		}
	}
	for id := range want {
		t.Errorf("gap %q missing from %s", id, raw)
	}
}
