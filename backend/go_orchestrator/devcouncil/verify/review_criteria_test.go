package verify_test

// llm_review is discharged by a recorded review of exactly the content under
// verification, made by reviewers that provably did not see the author's
// context, at least two of them, against the rubric the repository holds, with
// no blocker left open — and by nothing else.

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
)

const (
	reviewedSubject = "git-tree:aaaa"
	rubricBody      = "# Feature spec standard\nEvery failure mode is named.\n"
)

func rubricSum(body string) string {
	sum := sha256.Sum256([]byte(body))
	return hex.EncodeToString(sum[:])
}

func goodReview() dc.ReviewRecord {
	return dc.ReviewRecord{
		Schema: dc.ReviewRecordSchema, TaskID: "TASK-AC", AcceptanceCriterionID: "AC-1",
		SubjectHash: reviewedSubject, Harness: "test",
		CriterionSHA256: dc.CriterionDigest(dc.AcceptanceCriterion{ID: "AC-1", Description: "adds", Method: dc.VerifyLLMReview}),
		Rubric:          dc.ReviewRubric{Path: "docs/rubric.md", SHA256: rubricSum(rubricBody)},
		Isolation:       dc.ReviewIsolation{Probe: dc.IsolationPassed},
		Reviewers: []dc.ReviewerRun{
			{ID: "a", Persona: "critic", Status: dc.ReviewerCompleted},
			{ID: "b", Persona: "critic", Status: dc.ReviewerCompleted},
		},
	}
}

// reviewInput is a task whose one criterion is llm_review, with passing
// commands (which must never be what discharges it) and the given reviews of
// it, newest first, read against a repository holding the rubric.
func reviewInput(t *testing.T, required bool, reviews ...store.StoredReview) verify.Input {
	t.Helper()
	root := t.TempDir()
	if err := os.MkdirAll(filepath.Join(root, "docs"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "docs/rubric.md"), []byte(rubricBody), 0o600); err != nil {
		t.Fatal(err)
	}
	in := criterionInput(dc.VerifyLLMReview, required, 0, "go test ./...")
	in.Reviews = verify.ReviewEvidence{
		Subject:     reviewedSubject,
		ByCriterion: map[string][]store.StoredReview{"AC-1": reviews},
		Root:        root,
	}
	return in
}

func stored(id int64, r dc.ReviewRecord) store.StoredReview {
	return store.StoredReview{ID: id, SubjectHash: r.SubjectHash, RecordedAt: "2026-10-25T00:00:00Z", Record: r}
}

func run(t *testing.T, in verify.Input) (gaps []verify.Gap, status string) {
	t.Helper()
	gaps, meta := verify.Run(context.Background(), in)
	status, _ = verify.StatusFromGaps(gaps, meta.GateMode)
	return gaps, status
}

func TestLLMReviewIsDischargedOnlyByAReviewOfThisContent(t *testing.T) {
	gaps, status := run(t, reviewInput(t, true, stored(1, goodReview())))
	if status != "verified" || len(gapsOfType(gaps, "llm_review_unproven")) != 0 {
		t.Fatalf("a sound review of this content did not discharge: %s %+v", status, gaps)
	}
	if len(gapsOfType(gaps, "coarse_acceptance_proof")) != 0 {
		t.Errorf("a review is per-criterion evidence, not a coarse command proof: %+v", gaps)
	}
	if len(gapsOfType(gaps, "llm_review_findings")) != 0 {
		t.Errorf("a review with no findings left a findings gap: %+v", gaps)
	}
}

// Each way a review can fail to vouch for this content blocks, and the gap
// says which.
func TestAReviewThatCannotVouchForThisContentBlocks(t *testing.T) {
	stale := goodReview()
	stale.SubjectHash = "git-tree:bbbb"
	cases := map[string]struct {
		in   func(t *testing.T) verify.Input
		want string
	}{
		"no review": {func(t *testing.T) verify.Input { return reviewInput(t, true) }, "no review of this content"},
		"only a review of other content": {func(t *testing.T) verify.Input {
			return reviewInput(t, true, stored(3, stale))
		}, "reviewed git-tree:bbbb"},
		"isolation failed": {func(t *testing.T) verify.Input {
			r := goodReview()
			r.Isolation = dc.ReviewIsolation{Probe: dc.IsolationFailed, Detail: "child read CLAUDE.md"}
			return reviewInput(t, true, stored(1, r))
		}, "isolation probe failed"},
		"isolation not run": {func(t *testing.T) verify.Input {
			r := goodReview()
			r.Isolation.Probe = dc.IsolationNotRun
			return reviewInput(t, true, stored(1, r))
		}, "isolation probe not_run"},
		"one reviewer finished": {func(t *testing.T) verify.Input {
			r := goodReview()
			r.Reviewers[1].Status = dc.ReviewerFailed
			return reviewInput(t, true, stored(1, r))
		}, "1 of 2 reviewers completed"},
		"open blocker": {func(t *testing.T) verify.Input {
			r := goodReview()
			r.Findings = []dc.ReviewFinding{{ID: "F1", Severity: dc.SeverityBlocker, Summary: "no retry bound", Resolution: dc.ResolutionOpen}}
			return reviewInput(t, true, stored(1, r))
		}, "F1 (no retry bound)"},
		"rubric changed": {func(t *testing.T) verify.Input {
			r := goodReview()
			r.Rubric.SHA256 = rubricSum("an easier rubric")
			return reviewInput(t, true, stored(1, r))
		}, "not the one reviewed against"},
		"rubric missing": {func(t *testing.T) verify.Input {
			r := goodReview()
			r.Rubric.Path = "docs/gone.md"
			return reviewInput(t, true, stored(1, r))
		}, "cannot be read"},
		"rubric outside the repository": {func(t *testing.T) verify.Input {
			r := goodReview()
			r.Rubric.Path = "../rubric.md"
			return reviewInput(t, true, stored(1, r))
		}, "not a clean path inside"},
		"rubric absolute": {func(t *testing.T) verify.Input {
			r := goodReview()
			r.Rubric.Path = "/etc/hosts"
			return reviewInput(t, true, stored(1, r))
		}, "not a clean path inside"},
		"rubric symlinked out": {func(t *testing.T) verify.Input {
			outside := filepath.Join(t.TempDir(), "rubric.md")
			if err := os.WriteFile(outside, []byte(rubricBody), 0o600); err != nil {
				t.Fatal(err)
			}
			r := goodReview()
			r.Rubric.Path = "docs/linked.md"
			in := reviewInput(t, true, stored(1, r))
			if err := os.Symlink(outside, filepath.Join(in.Reviews.Root, "docs/linked.md")); err != nil {
				t.Fatal(err)
			}
			return in
		}, "resolves outside the repository"},
		"newest matching row unreadable": {func(t *testing.T) verify.Input {
			return reviewInput(t, true,
				store.StoredReview{ID: 2, SubjectHash: reviewedSubject, Err: errors.New("tampered")},
				stored(1, goodReview()))
		}, "review 2 of this content cannot be read"},
		"reviews unreadable": {func(t *testing.T) verify.Input {
			in := reviewInput(t, true, stored(1, goodReview()))
			in.Reviews.ReadErr = errors.New("dcstore is schema 10")
			return in
		}, "dcstore is schema 10"},
		"content unnamed": {func(t *testing.T) verify.Input {
			in := reviewInput(t, true, stored(1, goodReview()))
			in.Reviews.Subject, in.Reviews.SubjectErr = "", errors.New("not a git repository")
			return in
		}, "not a git repository"},
		"criterion re-planned after the review": {func(t *testing.T) verify.Input {
			in := reviewInput(t, true, stored(1, goodReview()))
			in.Requirements.Requirements[0].AcceptanceCriteria[0].Description = "adds, and is fast"
			return in
		}, "re-planned after the review"},
		"content changed while the commands ran": {func(t *testing.T) verify.Input {
			in := reviewInput(t, true, stored(1, goodReview()))
			in.Reviews.Recheck = func() (string, error) { return "tree-sha256:bbbb", nil }
			return in
		}, "changed while the verification commands ran"},
		"content could not be named again": {func(t *testing.T) verify.Input {
			in := reviewInput(t, true, stored(1, goodReview()))
			in.Reviews.Recheck = func() (string, error) { return "", errors.New("git vanished") }
			return in
		}, "git vanished"},
		"a record that does not validate": {func(t *testing.T) verify.Input {
			r := goodReview()
			r.Reviewers[1].ID = r.Reviewers[0].ID
			return reviewInput(t, true, stored(1, r))
		}, "not a valid record"},
		"a record filed under another subject": {func(t *testing.T) verify.Input {
			r := goodReview()
			r.SubjectHash = "tree-sha256:cccc"
			s := stored(1, r)
			s.SubjectHash = reviewedSubject
			return reviewInput(t, true, s)
		}, "is filed under"},
		"a record naming another task": {func(t *testing.T) verify.Input {
			r := goodReview()
			r.TaskID = "TASK-OTHER"
			return reviewInput(t, true, stored(1, r))
		}, "is filed under"},
		"no evidence read at all": {func(t *testing.T) verify.Input {
			in := criterionInput(dc.VerifyLLMReview, true, 0, "go test ./...")
			return in
		}, "was not named"},
	}
	for name, c := range cases {
		t.Run(name, func(t *testing.T) {
			gaps, status := run(t, c.in(t))
			unproven := gapsOfType(gaps, "llm_review_unproven")
			if len(unproven) != 1 || !unproven[0].Blocking {
				t.Fatalf("want one blocking llm_review_unproven, got %+v", gaps)
			}
			if status != "blocked" {
				t.Errorf("status = %s", status)
			}
			if evidence := strings.Join(unproven[0].Evidence, " | "); !strings.Contains(evidence, c.want) {
				t.Errorf("evidence %q does not say %q", evidence, c.want)
			}
			g := unproven[0]
			if g.AcceptanceCriterionID == nil || *g.AcceptanceCriterionID != "AC-1" ||
				g.ExpectedVerificationMethod == nil || *g.ExpectedVerificationMethod != "llm_review" ||
				g.RequirementID == nil || *g.RequirementID != "REQ-1" {
				t.Errorf("gap does not name the criterion, requirement and method: %+v", g)
			}
			if next := verify.NextActionFor(g); !strings.Contains(next.Action, "devcouncil review record") {
				t.Errorf("the next action does not say who records a review: %q", next.Action)
			}
		})
	}
}

// What a discharging review set aside or left open below blocker is reported
// for a person, without blocking.
func TestFindingsADischargingReviewLeftAreReported(t *testing.T) {
	r := goodReview()
	r.Findings = []dc.ReviewFinding{
		{ID: "F1", Severity: dc.SeverityBlocker, Summary: "race", Resolution: dc.ResolutionFixed},
		{ID: "F2", Severity: dc.SeverityBlocker, Summary: "unbounded", Resolution: dc.ResolutionContested, Note: "bounded at queue.go:40"},
		{ID: "F3", Severity: dc.SeverityMajor, Summary: "unstated SLO", Resolution: dc.ResolutionOpen},
		{ID: "F4", Severity: dc.SeverityNit, Summary: "typo", Resolution: dc.ResolutionOpen},
	}
	gaps, status := run(t, reviewInput(t, true, stored(1, r)))
	if status != "verified" {
		t.Fatalf("status = %s: %+v", status, gaps)
	}
	findings := gapsOfType(gaps, "llm_review_findings")
	if len(findings) != 1 || findings[0].Blocking {
		t.Fatalf("want one non-blocking findings gap: %+v", gaps)
	}
	evidence := strings.Join(findings[0].Evidence, " | ")
	for _, want := range []string{"contested blocker F2", "bounded at queue.go:40", "open major F3"} {
		if !strings.Contains(evidence, want) {
			t.Errorf("findings evidence %q lacks %q", evidence, want)
		}
	}
	for _, absent := range []string{"F1", "F4"} {
		if strings.Contains(evidence, absent) {
			t.Errorf("findings evidence %q names %s, which needs no one", evidence, absent)
		}
	}
}

// An optional criterion's missing review is reported, not blocking — the
// same rule every method follows.
func TestAnOptionalReviewCriterionDoesNotBlock(t *testing.T) {
	gaps, status := run(t, reviewInput(t, false))
	unproven := gapsOfType(gaps, "llm_review_unproven")
	if len(unproven) != 1 || unproven[0].Blocking {
		t.Fatalf("want one non-blocking gap: %+v", gaps)
	}
	if status != "verified" {
		t.Errorf("status = %s", status)
	}
}

// One id defined as llm_review by one requirement and as unit_test by another
// is ambiguous, and passing commands must not settle it by discharging the
// unit_test twin. (Adversarial round on 48a66ac3.) The same definition twice
// is one criterion.
func TestACriterionDefinedTwoWaysBlocks(t *testing.T) {
	for _, order := range [][2]dc.VerificationMethod{
		{dc.VerifyLLMReview, dc.VerifyUnitTest},
		{dc.VerifyUnitTest, dc.VerifyLLMReview},
	} {
		in := criterionInput(order[0], true, 0, "go test ./...")
		in.Requirements.Requirements = append(in.Requirements.Requirements, dc.Requirement{
			ID: "REQ-2", Priority: dc.PriorityHigh, Source: dc.SourceUser,
			AcceptanceCriteria: []dc.AcceptanceCriterion{{ID: "AC-1", Description: "adds", Method: order[1], Required: true}},
		})
		gaps, status := run(t, in)
		if status != "blocked" {
			t.Errorf("%v: status %s, want blocked: %+v", order, status, gaps)
		}
		found := false
		for _, g := range gapsOfType(gaps, "acceptance_criteria_unproven") {
			if g.Blocking && strings.Contains(strings.Join(g.Evidence, " "), "REQ-2") {
				found = true
			}
		}
		if !found {
			t.Errorf("%v: no blocking gap naming both definitions: %+v", order, gaps)
		}
	}
	same := criterionInput(dc.VerifyUnitTest, true, 0, "go test ./...")
	same.Requirements.Requirements = append(same.Requirements.Requirements, dc.Requirement{
		ID: "REQ-2", Priority: dc.PriorityHigh, Source: dc.SourceUser,
		AcceptanceCriteria: same.Requirements.Requirements[0].AcceptanceCriteria,
	})
	if _, status := run(t, same); status != "verified" {
		t.Errorf("one criterion defined identically twice did not verify: %s", status)
	}
}

func TestReviewsNeededNamesOnlyClaimedReviewCriteria(t *testing.T) {
	linked := store.LinkedRequirements{Requirements: []dc.Requirement{{
		ID: "REQ-1", AcceptanceCriteria: []dc.AcceptanceCriterion{
			{ID: "AC-1", Method: dc.VerifyLLMReview},
			{ID: "AC-2", Method: dc.VerifyUnitTest},
			{ID: "AC-3", Method: dc.VerifyLLMReview},
		},
	}}}
	got := verify.ReviewsNeeded([]string{"AC-1", "AC-2"}, linked)
	if len(got) != 1 || got[0] != "AC-1" {
		t.Fatalf("got %v, want [AC-1]: AC-3 is not claimed, AC-2 is not a review", got)
	}
	if verify.ReviewsNeeded(nil, linked) != nil {
		t.Error("a task claiming nothing needs reviews")
	}
}
