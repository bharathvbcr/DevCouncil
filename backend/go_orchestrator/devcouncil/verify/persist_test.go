package verify

import (
	"context"
	"path/filepath"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// TestPersistKeepsACriterionGapsLinkage drives a gap built by the criteria gate
// through Persist into a real dcstore and reads it back the way
// devcouncil_get_gaps does.
//
// toStoreGaps copied a gap's identity and text into store.GapRow but not its
// requirement, criterion or expected method, so an acceptance_criteria_unproven
// gap was persisted — and later reported — without saying which criterion it
// was about. The store round trip is tested in dc/store; this pins the mapping
// in between, which is the step that dropped them.
func TestPersistKeepsACriterionGapsLinkage(t *testing.T) {
	ctx := context.Background()
	client := store.New(testsupport.DCStore(t), filepath.Join(t.TempDir(), "state.sqlite"))

	req, method := "REQ-3", "unit_test"
	gap := unprovenCriterion("TASK-009", "AC-3.1", &req, &method, true,
		"AC-3.1 has no passing evidence", []string{"no commands"})
	if err := Persist(ctx, client, "TASK-009", []Gap{gap}, runMeta{Sandbox: SandboxLocal}, "failed"); err != nil {
		t.Fatalf("Persist: %v", err)
	}

	rows, _, err := client.Gaps(ctx, "TASK-009")
	if err != nil {
		t.Fatalf("Gaps: %v", err)
	}
	if len(rows) != 1 {
		t.Fatalf("gaps = %+v, want the one persisted", rows)
	}
	got := rows[0]
	for _, c := range []struct {
		field string
		got   *string
		want  string
	}{
		{"requirement_id", got.RequirementID, req},
		{"acceptance_criterion_id", got.AcceptanceCriterionID, "AC-3.1"},
		{"expected_verification_method", got.ExpectedVerificationMethod, method},
	} {
		if c.got == nil || *c.got != c.want {
			value := "<nil>"
			if c.got != nil {
				value = *c.got
			}
			t.Errorf("%s = %s, want %s", c.field, value, c.want)
		}
	}
}
