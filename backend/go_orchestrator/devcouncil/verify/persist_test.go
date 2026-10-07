package verify

import (
	"context"
	"fmt"
	"path/filepath"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// TestPersistKeepsAGapsLinkageAndLocation drives a gap built by the criteria
// gate through Persist into a real dcstore and reads it back the way
// devcouncil_get_gaps does.
//
// toStoreGaps copied a gap's identity and text into store.GapRow but not its
// requirement, criterion, expected method, file, line or suggested command, so
// an acceptance_criteria_unproven gap was persisted — and later reported —
// without saying which criterion it was about, and a file-scoped gap without
// saying where. The store round trip is tested in dc/store; this pins the
// mapping in between, which is the step that dropped them.
func TestPersistKeepsAGapsLinkageAndLocation(t *testing.T) {
	ctx := context.Background()
	client := store.New(testsupport.DCStore(t), filepath.Join(t.TempDir(), "state.sqlite"))

	req, method := "REQ-3", "unit_test"
	gap := unprovenCriterion("TASK-009", "AC-3.1", &req, &method, true,
		"AC-3.1 has no passing evidence", []string{"no commands"})
	// The criteria gate sets no location; the command and rigor gates do. One
	// gap carrying all six is enough to pin the mapping, which is per field.
	file, line, cmd := "pkg/a.go", 17, "go test ./pkg"
	gap.File, gap.Line, gap.SuggestedCommand = &file, &line, &cmd
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
	str := func(p *string) string {
		if p == nil {
			return "<nil>"
		}
		return fmt.Sprintf("%q", *p)
	}
	gotLine := "<nil>"
	if got.Line != nil {
		gotLine = fmt.Sprint(*got.Line)
	}
	for _, c := range []struct{ field, got, want string }{
		{"requirement_id", str(got.RequirementID), `"REQ-3"`},
		{"acceptance_criterion_id", str(got.AcceptanceCriterionID), `"AC-3.1"`},
		{"expected_verification_method", str(got.ExpectedVerificationMethod), `"unit_test"`},
		{"file", str(got.File), `"pkg/a.go"`},
		{"line", gotLine, "17"},
		{"suggested_command", str(got.SuggestedCommand), `"go test ./pkg"`},
	} {
		if c.got != c.want {
			t.Errorf("%s = %s, want %s", c.field, c.got, c.want)
		}
	}
}
