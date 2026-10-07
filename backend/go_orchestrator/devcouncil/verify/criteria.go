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

// criterionExecutors maps each method verify can discharge to its executor.
//
// manual is absent on purpose: verify cannot observe a person checking
// something, so a required manual criterion blocks until it is re-planned or
// an executor that records a person's sign-off exists. llm_review is refused
// earlier, at decode (dc/requirement.go). A method added to
// dc.VerificationMethods must be added here or it lands in the unsupported
// branch; criteria_test.go walks the whole list.
var criterionExecutors = map[dc.VerificationMethod]criterionExecutor{
	dc.VerifyUnitTest:        executorCommands,
	dc.VerifyIntegrationTest: executorCommands,
	dc.VerifyStaticCheck:     executorCommands,
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
func dispatchCriteria(taskID string, claimed []string, linked store.LinkedRequirements, readErr error, commands []string, commandsPassed bool) []Gap {
	if len(claimed) == 0 {
		return nil
	}
	type located struct {
		requirementID string
		criterion     dc.AcceptanceCriterion
	}
	byID := make(map[string]located)
	for _, req := range linked.Requirements {
		for _, ac := range req.AcceptanceCriteria {
			byID[ac.ID] = located{requirementID: req.ID, criterion: ac}
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
