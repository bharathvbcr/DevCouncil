package store

import (
	"context"
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
