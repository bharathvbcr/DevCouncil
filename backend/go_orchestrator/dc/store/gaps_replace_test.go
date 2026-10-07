package store

import (
	"context"
	"fmt"
	"testing"
)

// TestGapsReplacePersistsAGapItWasGiven is the regression test for a command
// that had never once completed.
//
// GapsReplace spells a replacement as `gaps-clear` followed by one `gap-upsert`
// per gap. `gap-upsert`'s handler reads ten flags — severity, gap-type,
// description, recommended-fix, evidence, blocking, file, line,
// suggested-command, expected-verification-method — and none of them was listed
// in dcstore's KNOWN_FLAGS, so the argument parser refused the argv before the
// handler ran. Two of the ten are required, so no argument vector reached it at
// all. Anything that had got past the flags would then have died on the reply:
// gap-upsert answered with a string under `id`, the same key evidence-append
// uses for an integer row number, in a Go envelope that decodes every reply on
// this boundary.
//
// verify/orchestrate.go calls GapsReplace after every verification, so
// persisting a task's gaps was failing for as long as both existed. Nothing
// caught it because every existing test that drives GapsReplace passes an empty
// list, which never enters the upsert loop — so this asserts the non-empty
// case, which is the only one that was broken.
func TestGapsReplacePersistsAGapItWasGiven(t *testing.T) {
	c := client(t)
	ctx := context.Background()

	want := GapRow{
		ID:             "TASK-001-STUB-1",
		Severity:       "high",
		GapType:        "stub_detected",
		TaskID:         "TASK-001",
		Description:    "added code whose body is `todo!()`",
		RecommendedFix: "Replace the placeholder with a real implementation.",
		Blocking:       true,
		EvidenceJSON:   []byte(`["src/a.rs:4: todo!()"]`),
	}
	if err := c.GapsReplace(ctx, "TASK-001", []GapRow{want}); err != nil {
		t.Fatalf("GapsReplace: %v", err)
	}

	got, truncated, err := c.Gaps(ctx, "TASK-001")
	if err != nil {
		t.Fatalf("Gaps: %v", err)
	}
	if truncated {
		t.Errorf("one gap reported truncated")
	}
	if len(got) != 1 {
		t.Fatalf("gaps = %+v, want the one that was written", got)
	}

	// Every field, not just the id. The flags that were refused are exactly the
	// ones that carry a gap's content, so a fix that restored only the argv
	// would still be checked by an assertion on identity alone.
	if got[0].ID != want.ID {
		t.Errorf("id = %q, want %q", got[0].ID, want.ID)
	}
	if got[0].Severity != want.Severity {
		t.Errorf("severity = %q, want %q", got[0].Severity, want.Severity)
	}
	if got[0].GapType != want.GapType {
		t.Errorf("gap_type = %q, want %q", got[0].GapType, want.GapType)
	}
	if got[0].Description != want.Description {
		t.Errorf("description = %q, want %q", got[0].Description, want.Description)
	}
	if got[0].RecommendedFix != want.RecommendedFix {
		t.Errorf("recommended_fix = %q, want %q", got[0].RecommendedFix, want.RecommendedFix)
	}
	if !got[0].Blocking {
		t.Errorf("blocking = false; a gap written as blocking must come back blocking")
	}
	if string(got[0].EvidenceJSON) != string(want.EvidenceJSON) {
		t.Errorf("evidence = %s, want %s", got[0].EvidenceJSON, want.EvidenceJSON)
	}
}

// TestGapsReplaceKeepsAGapsLinkageAndLocation is the regression test for a gap
// losing what it is about, and where it is, on the way through the store.
//
// verify raises acceptance_criteria_unproven and unsupported_verification_method
// with a requirement, a criterion and the method that criterion expected, and
// most file-scoped gaps with a file, a line or a command to reproduce them.
// dcstore stored all six, but GapRow had no fields for them, GapsReplace sent no
// flags for them and `gaps` emitted none of them — so devcouncil_get_gaps
// reported a criterion gap that did not say which criterion, and a stub that did
// not say where.
//
// The unlinked gap is asserted too: an absent field must come back nil, not as
// an empty string or a zero, or "this gap is not about a criterion" and "it is
// about a criterion with no id" become the same answer.
func TestGapsReplaceKeepsAGapsLinkageAndLocation(t *testing.T) {
	c := client(t)
	ctx := context.Background()

	req, ac, method := "REQ-7", "AC-7.2", "integration_test"
	file, line, cmd := "src/a.go", 42, "go test ./a"
	linked := GapRow{
		ID: "TASK-003-AC", Severity: "high", GapType: "acceptance_criteria_unproven",
		TaskID: "TASK-003", Description: "AC-7.2 has no passing evidence",
		RecommendedFix: "prove it", Blocking: true, EvidenceJSON: []byte(`[]`),
		RequirementID: &req, AcceptanceCriterionID: &ac, ExpectedVerificationMethod: &method,
		File: &file, Line: &line, SuggestedCommand: &cmd,
	}
	unlinked := GapRow{
		ID: "TASK-003-STUB", Severity: "low", GapType: "stub_detected",
		TaskID: "TASK-003", Description: "stub", RecommendedFix: "fill it",
		EvidenceJSON: []byte(`[]`),
	}
	// A pointer to "" is a field with no value; it must be stored as NULL like
	// an absent one, not as an empty string that reads back as a named
	// criterion or a file called "".
	empty := ""
	emptyLinked := GapRow{
		ID: "TASK-003-EMPTY", Severity: "low", GapType: "acceptance_criteria_unproven",
		TaskID: "TASK-003", Description: "empty", RecommendedFix: "name it",
		EvidenceJSON:  []byte(`[]`),
		RequirementID: &empty, AcceptanceCriterionID: &empty, ExpectedVerificationMethod: &empty,
		File: &empty, SuggestedCommand: &empty,
	}
	if err := c.GapsReplace(ctx, "TASK-003", []GapRow{linked, unlinked, emptyLinked}); err != nil {
		t.Fatalf("GapsReplace: %v", err)
	}

	got, _, err := c.Gaps(ctx, "TASK-003")
	if err != nil {
		t.Fatalf("Gaps: %v", err)
	}
	byID := map[string]GapRow{}
	for _, g := range got {
		byID[g.ID] = g
	}
	if len(byID) != 3 {
		t.Fatalf("gaps = %+v, want the three that were written", got)
	}

	none := map[string]string{
		"requirement_id": "<nil>", "acceptance_criterion_id": "<nil>",
		"expected_verification_method": "<nil>", "file": "<nil>", "line": "<nil>",
		"suggested_command": "<nil>",
	}
	for id, want := range map[string]map[string]string{
		linked.ID: {
			"requirement_id": `"REQ-7"`, "acceptance_criterion_id": `"AC-7.2"`,
			"expected_verification_method": `"integration_test"`, "file": `"src/a.go"`,
			"line": "42", "suggested_command": `"go test ./a"`,
		},
		unlinked.ID:    none,
		emptyLinked.ID: none,
	} {
		have := gapFields(byID[id])
		for field, w := range want {
			if have[field] != w {
				t.Errorf("%s: %s = %s, want %s", id, field, have[field], w)
			}
		}
	}
}

// gapFields renders a gap's optional fields so nil and a value never print the
// same: nil is <nil>, a string is quoted.
func gapFields(g GapRow) map[string]string {
	str := func(p *string) string {
		if p == nil {
			return "<nil>"
		}
		return fmt.Sprintf("%q", *p)
	}
	line := "<nil>"
	if g.Line != nil {
		line = fmt.Sprint(*g.Line)
	}
	return map[string]string{
		"requirement_id":               str(g.RequirementID),
		"acceptance_criterion_id":      str(g.AcceptanceCriterionID),
		"expected_verification_method": str(g.ExpectedVerificationMethod),
		"file":                         str(g.File),
		"line":                         line,
		"suggested_command":            str(g.SuggestedCommand),
	}
}

// TestGapsReplaceClearsWhatItReplaces is the other half of "replace".
//
// Without it the test above is satisfied by an implementation that only ever
// appends, and a task's report would accumulate every gap it had ever had.
func TestGapsReplaceClearsWhatItReplaces(t *testing.T) {
	c := client(t)
	ctx := context.Background()

	first := GapRow{
		ID: "TASK-002-A", Severity: "high", GapType: "stub_detected",
		TaskID: "TASK-002", Description: "a", RecommendedFix: "fix a",
		Blocking: true, EvidenceJSON: []byte(`[]`),
	}
	second := GapRow{
		ID: "TASK-002-B", Severity: "low", GapType: "diff_not_exercised",
		TaskID: "TASK-002", Description: "b", RecommendedFix: "fix b",
		Blocking: false, EvidenceJSON: []byte(`[]`),
	}
	if err := c.GapsReplace(ctx, "TASK-002", []GapRow{first}); err != nil {
		t.Fatalf("first: %v", err)
	}
	if err := c.GapsReplace(ctx, "TASK-002", []GapRow{second}); err != nil {
		t.Fatalf("second: %v", err)
	}

	got, _, err := c.Gaps(ctx, "TASK-002")
	if err != nil {
		t.Fatalf("Gaps: %v", err)
	}
	if len(got) != 1 || got[0].ID != second.ID {
		t.Fatalf("gaps = %+v, want only the replacement set", got)
	}
}
