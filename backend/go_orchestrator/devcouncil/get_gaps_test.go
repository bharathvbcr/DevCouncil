package devcouncil_test

import (
	"context"
	"encoding/json"
	"path/filepath"
	"reflect"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// TestGetGapsReportsAGapsOptionalFields reads gaps back through
// devcouncil_get_gaps from a real store and checks the JSON an MCP client
// receives.
//
// The handler builds each gap field by field, so a field the store carries is
// still lost here unless it is named. An acceptance_criteria_unproven gap that
// does not say which requirement and criterion it is about, or which method was
// expected, tells an agent that something is unproven but not what; a gap with
// no file, line or command tells it something is wrong but not where, and one
// with no output paths leaves it to re-run a command to see why it failed.
//
// Beyond the values below, every field store.GapRow decodes must appear as a
// key in every gap, so a field added to the store row and forgotten here fails
// this test instead of disappearing from the tool.
func TestGetGapsReportsAGapsOptionalFields(t *testing.T) {
	ctx := context.Background()
	client := store.New(testsupport.DCStore(t), filepath.Join(t.TempDir(), "state.sqlite"))

	req, ac, method := "REQ-1", "AC-1.1", "integration_test"
	file, line, cmd := "src/a.py", 9, "pytest tests/test_a.py"
	stdout, stderr := "runs/r1/stdout.log", "runs/r1/stderr.log"
	if err := client.GapsReplace(ctx, "TASK-1", []store.GapRow{
		{
			ID: "G-AC", Severity: "high", GapType: "acceptance_criteria_unproven",
			TaskID: "TASK-1", Description: "AC-1.1 unproven", RecommendedFix: "prove it",
			Blocking: true, EvidenceJSON: []byte(`[]`),
			RequirementID: &req, AcceptanceCriterionID: &ac, ExpectedVerificationMethod: &method,
			File: &file, Line: &line, SuggestedCommand: &cmd,
			StdoutPath: &stdout, StderrPath: &stderr,
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
			"stdout_path":                  `"runs/r1/stdout.log"`,
			"stderr_path":                  `"runs/r1/stderr.log"`,
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
			"stdout_path":                  `null`,
			"stderr_path":                  `null`,
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
		rowType := reflect.TypeOf(store.GapRow{})
		for i := 0; i < rowType.NumField(); i++ {
			key := strings.Split(rowType.Field(i).Tag.Get("json"), ",")[0]
			if _, ok := gap[key]; !ok {
				t.Errorf("%s: store.GapRow.%s (%q) is not in the reply", id, rowType.Field(i).Name, key)
			}
		}
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

func TestGetGapsReportsRecurrenceTruncated(t *testing.T) {
	ctx := context.Background()
	client := store.New(testsupport.DCStore(t), filepath.Join(t.TempDir(), "state.sqlite"))
	reg := devcouncil.NewRegistry(t.TempDir(), client, nil)

	callGaps := func() (avail, trunc, gapsTrunc bool) {
		out, err := reg.Call(ctx, "devcouncil_get_gaps", map[string]any{"task_id": "TASK-1"})
		if err != nil {
			t.Fatalf("Call: %v", err)
		}
		raw, err := json.Marshal(out)
		if err != nil {
			t.Fatalf("marshal: %v", err)
		}
		var payload struct {
			OK                  bool `json:"ok"`
			RecurrenceAvailable bool `json:"recurrence_available"`
			RecurrenceTruncated bool `json:"recurrence_truncated"`
			Truncated           bool `json:"truncated"`
		}
		if err := json.Unmarshal(raw, &payload); err != nil {
			t.Fatalf("unmarshal %s: %v", raw, err)
		}
		if !payload.OK {
			t.Fatalf("payload not ok: %s", raw)
		}
		return payload.RecurrenceAvailable, payload.RecurrenceTruncated, payload.Truncated
	}

	makeBatch := func(start, count int) []store.GapRow {
		rows := make([]store.GapRow, count)
		for i := 0; i < count; i++ {
			rows[i] = store.GapRow{
				ID:             "G-" + itoaGap(start+i),
				Severity:       "low",
				GapType:        "lint",
				TaskID:         "TASK-1",
				Description:    "gap",
				RecommendedFix: "fix",
				EvidenceJSON:   []byte(`[]`),
			}
		}
		return rows
	}

	if err := client.GapsReplace(ctx, "TASK-1", makeBatch(0, 1)); err != nil {
		t.Fatalf("small GapsReplace: %v", err)
	}
	avail, recTrunc, gapsTrunc := callGaps()
	if !avail || recTrunc || gapsTrunc {
		t.Fatalf("small batch: avail=%v recTrunc=%v gapsTrunc=%v, want true/false/false", avail, recTrunc, gapsTrunc)
	}

	// dcstore caps gap-history at MAX_LIST_ROWS (10,000); 21 batches of 500
	// distinct gaps leave 10,500 history rows while gaps holds only 500.
	for batch := 0; batch < 21; batch++ {
		if err := client.GapsReplace(ctx, "TASK-1", makeBatch(batch*store.MaxGapsPerReplace+1, store.MaxGapsPerReplace)); err != nil {
			t.Fatalf("GapsReplace batch %d: %v", batch, err)
		}
	}
	avail, recTrunc, gapsTrunc = callGaps()
	if !avail || !recTrunc || gapsTrunc {
		t.Fatalf("large history: avail=%v recTrunc=%v gapsTrunc=%v, want true/true/false", avail, recTrunc, gapsTrunc)
	}
}

func itoaGap(n int) string {
	if n == 0 {
		return "0"
	}
	var digits [20]byte
	i := len(digits)
	for n > 0 {
		i--
		digits[i] = byte('0' + n%10)
		n /= 10
	}
	return string(digits[i:])
}
