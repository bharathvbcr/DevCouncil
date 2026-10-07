package verify

import (
	"context"
	"encoding/json"
	"fmt"
	"path/filepath"
	"reflect"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// TestPersistKeepsEveryGapField drives a gap with every field set through
// Persist into a real dcstore and reads it back the way devcouncil_get_gaps
// does.
//
// toStoreGaps copied a gap's identity and text into store.GapRow and nothing
// else, so requirement, criterion, expected method, file, line, suggested
// command and output paths were all lost between verify and the store — eight
// fields, found over three passes, each one only after someone went looking.
// This test is built so the next one cannot be: the fixture must set every
// field of Gap (checked by reflection, so a new field fails here until it is
// set), and every field must come back.
func TestPersistKeepsEveryGapField(t *testing.T) {
	ctx := context.Background()
	client := store.New(testsupport.DCStore(t), filepath.Join(t.TempDir(), "state.sqlite"))

	s := func(v string) *string { return &v }
	line := 17
	gap := Gap{
		ID:                         "GAP-TASK-009-ALL",
		Severity:                   "high",
		GapType:                    "acceptance_criteria_unproven",
		RequirementID:              s("REQ-3"),
		TaskID:                     "TASK-009",
		Description:                "AC-3.1 has no passing evidence",
		Evidence:                   []string{"exit 1", "2 failed"},
		RecommendedFix:             "make it pass",
		Blocking:                   true,
		File:                       s("pkg/a.go"),
		Line:                       &line,
		SuggestedCommand:           s("go test ./pkg"),
		AcceptanceCriterionID:      s("AC-3.1"),
		StdoutPath:                 s(".devcouncil/runs/r1/stdout.log"),
		StderrPath:                 s(".devcouncil/runs/r1/stderr.log"),
		ExpectedVerificationMethod: s("unit_test"),
	}
	v := reflect.ValueOf(gap)
	for i := 0; i < v.NumField(); i++ {
		if v.Field(i).IsZero() {
			t.Fatalf("Gap.%s is unset in the fixture: set it and assert it round-trips below",
				v.Type().Field(i).Name)
		}
	}

	if err := Persist(ctx, client, gap.TaskID, []Gap{gap}, runMeta{Sandbox: SandboxLocal}, "failed"); err != nil {
		t.Fatalf("Persist: %v", err)
	}
	rows, _, err := client.Gaps(ctx, gap.TaskID)
	if err != nil {
		t.Fatalf("Gaps: %v", err)
	}
	if len(rows) != 1 {
		t.Fatalf("gaps = %+v, want the one persisted", rows)
	}
	got := rows[0]

	var evidence []string
	if err := json.Unmarshal(got.EvidenceJSON, &evidence); err != nil {
		t.Fatalf("evidence_json %s: %v", got.EvidenceJSON, err)
	}
	str := func(p *string) string {
		if p == nil {
			return "<nil>"
		}
		return fmt.Sprintf("%q", *p)
	}
	num := func(p *int) string {
		if p == nil {
			return "<nil>"
		}
		return fmt.Sprint(*p)
	}
	// One row per Gap field. The count is checked against the struct so a
	// field added there without a row here fails rather than going unasserted.
	checks := []struct{ field, got, want string }{
		{"ID", got.ID, gap.ID},
		{"Severity", got.Severity, gap.Severity},
		{"GapType", got.GapType, gap.GapType},
		{"RequirementID", str(got.RequirementID), str(gap.RequirementID)},
		{"TaskID", got.TaskID, gap.TaskID},
		{"Description", got.Description, gap.Description},
		{"Evidence", fmt.Sprint(evidence), fmt.Sprint(gap.Evidence)},
		{"RecommendedFix", got.RecommendedFix, gap.RecommendedFix},
		{"Blocking", fmt.Sprint(got.Blocking), fmt.Sprint(gap.Blocking)},
		{"File", str(got.File), str(gap.File)},
		{"Line", num(got.Line), num(gap.Line)},
		{"SuggestedCommand", str(got.SuggestedCommand), str(gap.SuggestedCommand)},
		{"AcceptanceCriterionID", str(got.AcceptanceCriterionID), str(gap.AcceptanceCriterionID)},
		{"StdoutPath", str(got.StdoutPath), str(gap.StdoutPath)},
		{"StderrPath", str(got.StderrPath), str(gap.StderrPath)},
		{"ExpectedVerificationMethod", str(got.ExpectedVerificationMethod), str(gap.ExpectedVerificationMethod)},
	}
	if len(checks) != v.NumField() {
		t.Fatalf("%d checks for %d Gap fields: add a row for the new field", len(checks), v.NumField())
	}
	for i, c := range checks {
		if name := v.Type().Field(i).Name; c.field != name {
			t.Fatalf("check %d is for %s, but Gap's field %d is %s: keep the rows in field order", i, c.field, i, name)
		}
		if c.got != c.want {
			t.Errorf("%s = %s, want %s", c.field, c.got, c.want)
		}
	}
}
