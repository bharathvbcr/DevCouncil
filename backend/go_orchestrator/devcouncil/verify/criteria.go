package verify

// Acceptance-criterion dispatch.
//
// A task names the acceptance criteria it is accountable for, and each
// criterion names how it is to be proved (dc.VerificationMethod). Before this
// file the method was validated in dc/requirement.go and read by nothing: a
// criterion verified "by unit_test" was treated exactly like one with no
// method, and a task whose criteria nothing could discharge verified clean.
//
// Every method now resolves to one of two outcomes. Either it has an executor
// in criterionExecutors and is discharged by evidence this run produced, or
// it does not and the criterion becomes a blocking
// unsupported_verification_method gap. There is no third outcome where a
// criterion is simply not looked at.

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
)

// criterionExecutor names the evidence that discharges a criterion.
type criterionExecutor string

// executorCommands discharges a criterion when the task's verification
// commands (expected_tests, falling back to allowed_commands) all ran and
// passed. That is task-level evidence, not a check of this criterion in
// particular, so a criterion discharged this way is also reported as a
// non-blocking coarse_acceptance_proof.
const executorCommands criterionExecutor = "verification_commands"

// executorReview discharges a criterion from a recorded review
// (dc.ReviewRecord) of exactly the content under verification; see
// judgeReview. Passing commands never discharge it: a test that ran is not a
// reader who judged.
const executorReview criterionExecutor = "review_record"

// criterionExecutors maps each method verify can discharge to its executor.
//
// manual is absent on purpose: verify cannot observe a person checking
// something, so a required manual criterion blocks until it is re-planned or
// an executor that records a person's sign-off exists. A method added to
// dc.VerificationMethods must be added here or it lands in the unsupported
// branch; criteria_test.go walks the whole list.
var criterionExecutors = map[dc.VerificationMethod]criterionExecutor{
	dc.VerifyUnitTest:        executorCommands,
	dc.VerifyIntegrationTest: executorCommands,
	dc.VerifyStaticCheck:     executorCommands,
	dc.VerifyLLMReview:       executorReview,
}

// ReviewEvidence is what verify read to judge llm_review criteria.
//
// Every field that could not be filled carries its error rather than an empty
// value: "no review was recorded" and "the reviews could not be read" are
// different facts, and only the first is the agent's to fix.
type ReviewEvidence struct {
	// Subject is WorkingTreeSubject for the content under verification.
	Subject    string
	SubjectErr error
	// ByCriterion is each llm_review criterion's reviews, newest first.
	ByCriterion map[string][]store.StoredReview
	ReadErr     error
	// Root is where the rubric a review names is read from.
	Root string
}

// ReviewsNeeded names the claimed criteria that are llm_review criteria in
// linked,
// so a task without one costs no subject computation and no store read.
func ReviewsNeeded(claimed []string, linked store.LinkedRequirements) []string {
	want := make(map[string]bool, len(claimed))
	for _, id := range claimed {
		want[id] = true
	}
	var ids []string
	for _, req := range linked.Requirements {
		for _, ac := range req.AcceptanceCriteria {
			if ac.Method == dc.VerifyLLMReview && want[ac.ID] {
				ids = append(ids, ac.ID)
				want[ac.ID] = false
			}
		}
	}
	return ids
}

// LoadReviewEvidence names the content under verification and reads the
// reviews of the given criteria. Failures are carried in the result.
func LoadReviewEvidence(ctx context.Context, root string, client *store.Client, taskID string, criteria []string) ReviewEvidence {
	ev := ReviewEvidence{Root: root, ByCriterion: make(map[string][]store.StoredReview, len(criteria))}
	ev.Subject, ev.SubjectErr = WorkingTreeSubject(ctx, root)
	if client == nil {
		ev.ReadErr = fmt.Errorf("store unavailable")
		return ev
	}
	for _, id := range criteria {
		reviews, err := client.Reviews(ctx, taskID, id)
		if err != nil {
			ev.ReadErr = err
			return ev
		}
		ev.ByCriterion[id] = reviews
	}
	return ev
}

// maxRubricBytes bounds the rubric read. A standard is prose; a megabyte is
// past any of them.
const maxRubricBytes = 1 << 20

// judgeReview decides whether the reviews of one criterion discharge it for
// the content under verification. It returns the reasons it does not (empty
// when it does) and the record that discharged it.
//
// The review that counts is the newest whose subject is this content. An
// older review of other content is named in the reasons — it is the usual
// state after a fix — but never consulted. A newest-matching row that could
// not be read blocks rather than falling through to an older one: an
// unreadable record is not a missing one.
func judgeReview(criterion dc.AcceptanceCriterion, ev ReviewEvidence) (reasons []string, record *dc.ReviewRecord) {
	acID := criterion.ID
	if ev.ReadErr != nil {
		return []string{"the reviews could not be read: " + ev.ReadErr.Error()}, nil
	}
	if ev.SubjectErr != nil {
		return []string{"the content under verification could not be named, so no review can be " +
			"matched to it: " + ev.SubjectErr.Error()}, nil
	}
	if ev.Subject == "" {
		return []string{"the content under verification was not named, so no review can be matched to it"}, nil
	}
	reviews := ev.ByCriterion[acID]
	var match *store.StoredReview
	for i := range reviews {
		if reviews[i].SubjectHash == ev.Subject {
			match = &reviews[i]
			break
		}
	}
	if match == nil {
		why := "no review of this content (" + ev.Subject + ") is recorded"
		if len(reviews) > 0 {
			why += fmt.Sprintf("; the newest, review %d recorded %s, reviewed %s",
				reviews[0].ID, reviews[0].RecordedAt, reviews[0].SubjectHash)
		}
		return []string{why}, nil
	}
	if match.Err != nil {
		return []string{fmt.Sprintf("review %d of this content cannot be read: %v", match.ID, match.Err)}, nil
	}
	r := match.Record
	if want := dc.CriterionDigest(criterion); r.CriterionSHA256 != want {
		reasons = append(reasons, fmt.Sprintf("review %d answered a different version of %s (criterion "+
			"sha256 %s, now %s): it was re-planned after the review", match.ID, acID, r.CriterionSHA256, want))
	}
	if r.Isolation.Probe != dc.IsolationPassed {
		reasons = append(reasons, fmt.Sprintf("review %d: the isolation probe %s (%s), so the "+
			"reviewers may have seen the author's context", match.ID, r.Isolation.Probe, r.Isolation.Detail))
	}
	if n := r.CompletedReviewers(); n < dc.MinIndependentReviewers {
		reasons = append(reasons, fmt.Sprintf("review %d: %d of %d reviewers completed; at least %d "+
			"independent reviewers must", match.ID, n, len(r.Reviewers), dc.MinIndependentReviewers))
	}
	if why := checkRubric(ev.Root, r.Rubric); why != "" {
		reasons = append(reasons, fmt.Sprintf("review %d: %s", match.ID, why))
	}
	var blockers []string
	for _, f := range r.Findings {
		if f.Severity == dc.SeverityBlocker && f.Resolution == dc.ResolutionOpen {
			blockers = append(blockers, f.ID+" ("+f.Summary+")")
		}
	}
	if len(blockers) > 0 {
		reasons = append(reasons, fmt.Sprintf("review %d left blocker findings open: %s",
			match.ID, strings.Join(blockers, "; ")))
	}
	return reasons, &r
}

// checkRubric reports why the rubric a review names is not the one in the
// repository, or "" when it is: a repository-relative path inside root,
// reached without leaving root through a symlink, whose bytes hash to what
// the review recorded.
func checkRubric(root string, rubric dc.ReviewRubric) string {
	rel := filepath.FromSlash(rubric.Path)
	if filepath.IsAbs(rel) || rel != filepath.Clean(rel) || rel == "." ||
		strings.HasPrefix(rel, ".."+string(filepath.Separator)) || rel == ".." {
		return "the rubric path " + rubric.Path + " is not a clean path inside the repository"
	}
	realRoot, err := filepath.EvalSymlinks(root)
	if err != nil {
		return "the repository root cannot be resolved: " + err.Error()
	}
	realPath, err := filepath.EvalSymlinks(filepath.Join(root, rel))
	if err != nil {
		return "the rubric " + rubric.Path + " cannot be read: " + err.Error()
	}
	if inside, err := filepath.Rel(realRoot, realPath); err != nil || inside == ".." ||
		strings.HasPrefix(inside, ".."+string(filepath.Separator)) {
		return "the rubric " + rubric.Path + " resolves outside the repository"
	}
	f, err := os.Open(realPath)
	if err != nil {
		return "the rubric " + rubric.Path + " cannot be read: " + err.Error()
	}
	defer f.Close()
	h := sha256.New()
	n, err := io.Copy(h, io.LimitReader(f, maxRubricBytes+1))
	if err != nil {
		return "the rubric " + rubric.Path + " cannot be read: " + err.Error()
	}
	if n > maxRubricBytes {
		return fmt.Sprintf("the rubric %s is over %d bytes", rubric.Path, maxRubricBytes)
	}
	if got := hex.EncodeToString(h.Sum(nil)); got != rubric.SHA256 {
		return "the rubric " + rubric.Path + " is not the one reviewed against (sha256 " + got +
			", review recorded " + rubric.SHA256 + ")"
	}
	return ""
}

// reviewFindingsGap reports, without blocking, the findings a discharging
// review left for a person: open majors, and anything set aside.
func reviewFindingsGap(taskID, acID string, reqID *string, r *dc.ReviewRecord) *Gap {
	var evidence []string
	for _, f := range r.Findings {
		switch {
		case f.Resolution == dc.ResolutionContested:
			evidence = append(evidence, fmt.Sprintf("contested %s %s: %s — %s", f.Severity, f.ID, f.Summary, f.Note))
		case f.Resolution == dc.ResolutionOpen && f.Severity == dc.SeverityMajor:
			evidence = append(evidence, fmt.Sprintf("open major %s: %s", f.ID, f.Summary))
		}
	}
	if len(evidence) == 0 {
		return nil
	}
	method := string(dc.VerifyLLMReview)
	return &Gap{
		ID:       StableGapID(taskID, "REVIEWFIND", "ac:"+acID),
		Severity: "medium",
		GapType:  "llm_review_findings",
		TaskID:   taskID,
		Description: "The review discharging acceptance criterion " + acID +
			" left findings a person should read.",
		Evidence:                   evidence,
		RecommendedFix:             "Fix or answer each listed finding before the task is accepted.",
		Blocking:                   false,
		RequirementID:              reqID,
		AcceptanceCriterionID:      &acID,
		ExpectedVerificationMethod: &method,
	}
}

// CriterionExecutor reports the executor that discharges method, if any.
func CriterionExecutor(method dc.VerificationMethod) (string, bool) {
	exec, ok := criterionExecutors[method]
	return string(exec), ok
}

// dispatchCriteria turns the criteria a task claims into gaps.
//
// commandsPassed is true only when at least one verification command ran and
// every one of them passed. readErr is set when the task's requirements could
// not be read; then every claimed criterion blocks, because a criterion nobody
// could read is not one with nothing to prove.
func dispatchCriteria(taskID string, claimed []string, linked store.LinkedRequirements, readErr error, commands []string, commandsPassed bool, reviews ReviewEvidence) []Gap {
	if len(claimed) == 0 {
		return nil
	}
	type located struct {
		requirementID string
		criterion     dc.AcceptanceCriterion
	}
	// One id defined twice the same way is one criterion. Defined twice
	// differently — another method, text or required flag under the same id —
	// it is two claims with one name, and dispatching on either would let the
	// other pass unproven (an llm_review criterion discharged by a unit_test
	// twin's commands). It blocks instead.
	byID := make(map[string]located)
	conflicting := make(map[string][]string)
	for _, req := range linked.Requirements {
		for _, ac := range req.AcceptanceCriteria {
			prev, seen := byID[ac.ID]
			if !seen {
				byID[ac.ID] = located{requirementID: req.ID, criterion: ac}
				continue
			}
			if prev.criterion != ac {
				if len(conflicting[ac.ID]) == 0 {
					conflicting[ac.ID] = []string{prev.requirementID + " (" + string(prev.criterion.Method) + ")"}
				}
				conflicting[ac.ID] = append(conflicting[ac.ID], req.ID+" ("+string(ac.Method)+")")
			}
		}
	}

	var gaps []Gap
	var discharged []string
	for _, id := range claimed {
		acID := id
		if readErr != nil {
			gaps = append(gaps, unprovenCriterion(taskID, acID, nil, nil, true,
				"The task's requirements could not be read, so acceptance criterion "+acID+
					" cannot be dispatched on its verification method.",
				[]string{"requirements could not be read: " + readErr.Error()}))
			continue
		}
		if defs := conflicting[acID]; len(defs) > 0 {
			gaps = append(gaps, unprovenCriterion(taskID, acID, nil, nil, true,
				"Acceptance criterion "+acID+" is defined differently by more than one linked requirement, "+
					"so which of them the task must prove is ambiguous.",
				[]string{"defined by: " + strings.Join(defs, ", ")}))
			continue
		}
		found, ok := byID[acID]
		if !ok {
			evidence := []string{"no requirement linked to this task defines " + acID}
			if len(linked.Missing) > 0 {
				evidence = append(evidence, "linked requirements with no row: "+strings.Join(linked.Missing, ", "))
			}
			gaps = append(gaps, unprovenCriterion(taskID, acID, nil, nil, true,
				"Acceptance criterion "+acID+" is claimed by this task but defined by no requirement it links to.",
				evidence))
			continue
		}
		reqID := found.requirementID
		method := string(found.criterion.Method)
		executor, has := criterionExecutors[found.criterion.Method]
		switch {
		case !has:
			gaps = append(gaps, Gap{
				ID:       StableGapID(taskID, "ACMETHOD", "method:"+acID),
				Severity: "high",
				GapType:  "unsupported_verification_method",
				TaskID:   taskID,
				Description: "Acceptance criterion " + acID + " is verified by " + method +
					", which verify has no executor for; it has not been checked.",
				Evidence: []string{"verification_method: " + method},
				RecommendedFix: "Have a person confirm " + acID + " and re-plan it with a method verify " +
					"can discharge (unit_test, integration_test, static_check), or mark it required: false.",
				Blocking:                   found.criterion.Required,
				RequirementID:              &reqID,
				AcceptanceCriterionID:      &acID,
				ExpectedVerificationMethod: &method,
			})
		case executor == executorReview:
			reasons, record := judgeReview(found.criterion, reviews)
			if len(reasons) > 0 {
				gaps = append(gaps, Gap{
					ID:       StableGapID(taskID, "AC", "ac:"+acID),
					Severity: "high",
					GapType:  "llm_review_unproven",
					TaskID:   taskID,
					Description: "Acceptance criterion " + acID + " (llm_review) is not proved by a " +
						"recorded independent review of the content under verification.",
					Evidence: reasons,
					RecommendedFix: "Have the harness run the reviewers on this content and record the " +
						"review; resolve any open blocker it finds.",
					Blocking:                   found.criterion.Required,
					RequirementID:              &reqID,
					AcceptanceCriterionID:      &acID,
					ExpectedVerificationMethod: &method,
				})
			} else if g := reviewFindingsGap(taskID, acID, &reqID, record); g != nil {
				gaps = append(gaps, *g)
			}
		case executor == executorCommands && commandsPassed:
			discharged = append(discharged, acID)
		default:
			why := "a verification command failed or could not run"
			if len(commands) == 0 {
				why = "the task has no expected_tests or allowed_commands to run"
			}
			gaps = append(gaps, unprovenCriterion(taskID, acID, &reqID, &method, found.criterion.Required,
				"Acceptance criterion "+acID+" ("+method+") has no passing evidence: "+why+".",
				[]string{why}))
		}
	}
	if len(discharged) > 0 {
		gaps = append(gaps, Gap{
			ID:       StableGapID(taskID, "COARSEAC"),
			Severity: "low",
			GapType:  "coarse_acceptance_proof",
			TaskID:   taskID,
			Description: "Acceptance criteria " + strings.Join(discharged, ", ") +
				" are proven only by the task's verification commands passing, not by a per-criterion check.",
			Evidence:       []string{"commands: " + strings.Join(commands, "; ")},
			RecommendedFix: "Add a test that exercises each listed criterion specifically.",
			Blocking:       false,
		})
	}
	return gaps
}

func unprovenCriterion(taskID, acID string, reqID, method *string, blocking bool, description string, evidence []string) Gap {
	return Gap{
		ID:                         StableGapID(taskID, "AC", "ac:"+acID),
		Severity:                   "high",
		GapType:                    "acceptance_criteria_unproven",
		TaskID:                     taskID,
		Description:                description,
		Evidence:                   evidence,
		RecommendedFix:             "Provide a passing verification command that proves " + acID + ".",
		Blocking:                   blocking,
		RequirementID:              reqID,
		AcceptanceCriterionID:      &acID,
		ExpectedVerificationMethod: method,
	}
}
