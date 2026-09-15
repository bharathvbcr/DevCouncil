package store

import (
	"context"
	"testing"
)

// gapFor builds the row one verification would report.
func gapFor(taskID, id string) GapRow {
	return GapRow{
		ID:             taskID + "-" + id,
		Severity:       "high",
		GapType:        "stub_detected",
		TaskID:         taskID,
		Description:    "placeholder left in the diff",
		RecommendedFix: "implement it",
		Blocking:       true,
		EvidenceJSON:   []byte(`[]`),
	}
}

// TestAGapThatComesBackIsRecordedAcrossTheProcessBoundary drives the real
// dcstore binary, because the thing under test is the boundary.
//
// The Rust side has its own tests for the recurrence arithmetic. What those
// cannot show is that GapsReplace — which is *not* a single call to the Rust
// gaps_replace, but a gaps-clear followed by one gap-upsert per gap — produces
// the same history when it crosses two processes.
func TestAGapThatComesBackIsRecordedAcrossTheProcessBoundary(t *testing.T) {
	c := client(t)
	ctx := context.Background()

	// Run 1: the gap is reported. Run 2: clean, so something fixed it. Run 3:
	// it is back.
	if err := c.GapsReplace(ctx, "TASK-001", []GapRow{gapFor("TASK-001", "G1")}); err != nil {
		t.Fatalf("run 1: %v", err)
	}
	if err := c.GapsReplace(ctx, "TASK-001", nil); err != nil {
		t.Fatalf("run 2: %v", err)
	}
	if err := c.GapsReplace(ctx, "TASK-001", []GapRow{gapFor("TASK-001", "G1")}); err != nil {
		t.Fatalf("run 3: %v", err)
	}

	history, truncated, err := c.GapHistory(ctx, "TASK-001")
	if err != nil {
		t.Fatalf("gap-history: %v", err)
	}
	if truncated {
		t.Errorf("a three-run history reported truncated")
	}
	if len(history) != 1 {
		t.Fatalf("history = %+v, want one row", history)
	}
	row := history[0]
	if row.GapID != "TASK-001-G1" || row.TaskID != "TASK-001" {
		t.Errorf("history row identity = %+v", row)
	}
	if row.FirstSeenRun != 1 || row.LastSeenRun != 3 {
		t.Errorf("runs = %d..%d, want 1..3", row.FirstSeenRun, row.LastSeenRun)
	}
	if row.Occurrences != 2 {
		t.Errorf("occurrences = %d, want 2", row.Occurrences)
	}
	if row.Resurfaces != 1 {
		t.Errorf("resurfaces = %d, want 1; the clean run between the two "+
			"sightings is the whole signal", row.Resurfaces)
	}

	// The current-state table still answers only the present. The history was
	// added beside it, not in place of it.
	gaps, _, err := c.Gaps(ctx, "TASK-001")
	if err != nil {
		t.Fatalf("gaps: %v", err)
	}
	if len(gaps) != 1 {
		t.Errorf("gaps = %+v, want the one currently open", gaps)
	}
}

// TestAGapNobodyFixedNeverReportsAResurface is the control.
//
// Without it, the assertion above is satisfied by an implementation that counts
// every sighting as a resurface — which would make the signal meaningless while
// still being nonzero exactly when the test expects.
func TestAGapNobodyFixedNeverReportsAResurface(t *testing.T) {
	c := client(t)
	ctx := context.Background()

	for i := 0; i < 3; i++ {
		if err := c.GapsReplace(ctx, "TASK-002", []GapRow{gapFor("TASK-002", "G1")}); err != nil {
			t.Fatalf("run %d: %v", i+1, err)
		}
	}
	history, _, err := c.GapHistory(ctx, "TASK-002")
	if err != nil {
		t.Fatalf("gap-history: %v", err)
	}
	if len(history) != 1 {
		t.Fatalf("history = %+v", history)
	}
	if history[0].Occurrences != 3 {
		t.Errorf("occurrences = %d, want 3", history[0].Occurrences)
	}
	if history[0].Resurfaces != 0 {
		t.Errorf("resurfaces = %d for a gap reported by every run, want 0",
			history[0].Resurfaces)
	}
}

// TestGapHistoryIsEmptyForATaskThatHasNeverVerified pins the difference between
// "nothing has recurred" and "no verification has run".
//
// Both are an empty list here, and that is correct at this layer: the caller
// that needs to tell them apart reads the run counter through the gaps it
// receives, not through the absence of rows.
func TestGapHistoryIsEmptyForATaskThatHasNeverVerified(t *testing.T) {
	c := client(t)
	history, _, err := c.GapHistory(context.Background(), "TASK-NEVER")
	if err != nil {
		t.Fatalf("gap-history: %v", err)
	}
	if len(history) != 0 {
		t.Errorf("history = %+v, want empty", history)
	}
}
