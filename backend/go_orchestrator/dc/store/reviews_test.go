package store

import (
	"context"
	"os/exec"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

func reviewRecord(task, criterion, subject string) dc.ReviewRecord {
	return dc.ReviewRecord{
		Schema:                dc.ReviewRecordSchema,
		TaskID:                task,
		AcceptanceCriterionID: criterion,
		SubjectHash:           subject,
		Harness:               "test-harness",
		Rubric:                dc.ReviewRubric{Path: "docs/rubric.md", SHA256: strings.Repeat("b", 64)},
		Isolation:             dc.ReviewIsolation{Probe: dc.IsolationPassed},
		Reviewers: []dc.ReviewerRun{
			{ID: "a", Persona: "critic", Status: dc.ReviewerCompleted},
			{ID: "b", Persona: "critic", Status: dc.ReviewerCompleted},
		},
		Findings: []dc.ReviewFinding{},
	}
}

// A review the harness records is the one verify reads back, field for field,
// newest first, filed under the task and criterion it names.
func TestAReviewRoundTripsThroughTheStore(t *testing.T) {
	c, _ := plantTaskWithLinks(t, `[]`, `["AC-1"]`)
	ctx := context.Background()
	first := reviewRecord("TASK-1", "AC-1", "git-tree:01")
	second := reviewRecord("TASK-1", "AC-1", "git-tree:02")
	second.Findings = []dc.ReviewFinding{{ID: "F1", Severity: dc.SeverityMajor, Summary: "s", Resolution: dc.ResolutionOpen}}
	other := reviewRecord("TASK-1", "AC-2", "git-tree:01")
	var ids []int64
	for _, r := range []dc.ReviewRecord{first, other, second} {
		id, err := c.ReviewAppend(ctx, r)
		if err != nil {
			t.Fatal(err)
		}
		ids = append(ids, id)
	}

	reviews, err := c.Reviews(ctx, "TASK-1", "AC-1")
	if err != nil {
		t.Fatal(err)
	}
	if len(reviews) != 2 || reviews[0].ID != ids[2] || reviews[1].ID != ids[0] {
		t.Fatalf("reviews = %+v, want ids %d then %d", reviews, ids[2], ids[0])
	}
	if reviews[0].Err != nil || reviews[0].Record.SubjectHash != "git-tree:02" ||
		len(reviews[0].Record.Findings) != 1 || reviews[0].RecordedAt == "" {
		t.Errorf("newest = %+v", reviews[0])
	}
	all, err := c.Reviews(ctx, "TASK-1", "")
	if err != nil || len(all) != 3 {
		t.Fatalf("all = %d, %v", len(all), err)
	}
}

// The writer refuses what every reader would refuse, before the store sees it,
// and the store refuses a review for a task that does not exist.
func TestAReviewNothingCouldReadIsNeverStored(t *testing.T) {
	c, _ := plantTaskWithLinks(t, `[]`, `["AC-1"]`)
	ctx := context.Background()
	bad := reviewRecord("TASK-1", "AC-1", "git-tree:01")
	bad.Reviewers = nil
	if _, err := c.ReviewAppend(ctx, bad); err == nil || !strings.Contains(err.Error(), "0 reviewers") {
		t.Errorf("err = %v", err)
	}
	if _, err := c.ReviewAppend(ctx, reviewRecord("NO-SUCH-TASK", "AC-1", "git-tree:01")); err == nil ||
		!strings.Contains(err.Error(), "no task") {
		t.Errorf("err = %v, want the store's refusal of an unknown task", err)
	}
	reviews, err := c.Reviews(ctx, "TASK-1", "")
	if err != nil || len(reviews) != 0 {
		t.Fatalf("reviews = %+v, %v", reviews, err)
	}
}

// A row whose bytes were altered after the write is reported with its reason,
// in its place, never dropped so an older row stands in for it.
func TestATamperedReviewRowIsReportedNotSkipped(t *testing.T) {
	c, db := plantTaskWithLinks(t, `[]`, `["AC-1"]`)
	ctx := context.Background()
	if _, err := c.ReviewAppend(ctx, reviewRecord("TASK-1", "AC-1", "git-tree:01")); err != nil {
		t.Fatal(err)
	}
	if _, err := c.ReviewAppend(ctx, reviewRecord("TASK-1", "AC-1", "git-tree:02")); err != nil {
		t.Fatal(err)
	}
	// The newest row: its record now claims another criterion, and the next
	// one's record is not JSON the store would have accepted.
	sqlite, err := exec.LookPath("sqlite3")
	if err != nil {
		testsupport.Unavailable(t, "sqlite3 is not on PATH, so no review row can be altered")
	}
	// #nosec G204 -- sqlite is resolved by exec.LookPath, db is this test's
	// own t.TempDir, and the statement is this file's literal.
	cmd := exec.CommandContext(t.Context(), sqlite, db,
		`UPDATE reviews SET record_json = replace(record_json, '"AC-1"', '"AC-9"') WHERE subject_hash = 'git-tree:02';`+
			`UPDATE reviews SET record_json = '{"schema":' WHERE subject_hash = 'git-tree:01';`)
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("altering reviews: %v\n%s", err, out)
	}
	reviews, err := c.Reviews(ctx, "TASK-1", "AC-1")
	if err != nil {
		t.Fatal(err)
	}
	if len(reviews) != 2 {
		t.Fatalf("reviews = %+v", reviews)
	}
	if reviews[0].Err == nil || !strings.Contains(reviews[0].Err.Error(), "stored under") {
		t.Errorf("newest: %v, want a refusal of a record filed under another criterion", reviews[0].Err)
	}
	if reviews[1].Err == nil || !strings.Contains(reviews[1].Err.Error(), "does not parse") {
		t.Errorf("older: %v, want the store's record_error carried", reviews[1].Err)
	}
}
