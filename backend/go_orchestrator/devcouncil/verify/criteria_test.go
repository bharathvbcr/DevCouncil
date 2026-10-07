package verify_test

// A requirement's verification method used to be validated and then ignored:
// nothing in verify read it, so a criterion verified "by unit_test" was treated
// exactly like one with no method, and a task linked to criteria nobody could
// discharge verified clean. These drive verify.Run with the criteria a task
// links to and check that every method is either discharged by evidence verify
// actually produced, or reported as a blocking gap.

import (
	"context"
	"errors"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
)

func criterionInput(method dc.VerificationMethod, required bool, commandExit int, commands ...string) verify.Input {
	return verify.Input{
		Task: &store.Task{
			ID:                     "TASK-AC",
			RequirementIDs:         []string{"REQ-1"},
			AcceptanceCriterionIDs: []string{"AC-1"},
			PlannedFiles:           []dc.PlannedFile{{Path: "src/a.go", AllowedChange: dc.ChangeModify}},
		},
		GateMode:     "enforce",
		ChangedFiles: []string{"src/a.go"},
		DiffContent:  "diff --git a/src/a.go b/src/a.go\n",
		WorkPresent:  true,
		Commands:     commands,
		RunCommand: func(string) verify.CommandOutcome {
			return verify.CommandOutcome{ExitCode: commandExit}
		},
		Requirements: store.LinkedRequirements{Requirements: []dc.Requirement{{
			ID: "REQ-1", Priority: dc.PriorityHigh, Source: dc.SourceUser,
			AcceptanceCriteria: []dc.AcceptanceCriterion{
				{ID: "AC-1", Description: "adds", Method: method, Required: required},
			},
		}}},
	}
}

func gapsOfType(gaps []verify.Gap, gapType string) []verify.Gap {
	var out []verify.Gap
	for _, g := range gaps {
		if g.GapType == gapType {
			out = append(out, g)
		}
	}
	return out
}

func TestEveryVerificationMethodIsDischargedOrBlocks(t *testing.T) {
	for _, method := range dc.VerificationMethods() {
		t.Run(string(method), func(t *testing.T) {
			gaps, meta := verify.Run(context.Background(), criterionInput(method, true, 0, "go test ./..."))
			unsupported := gapsOfType(gaps, "unsupported_verification_method")
			unproven := gapsOfType(gaps, "acceptance_criteria_unproven")
			coarse := gapsOfType(gaps, "coarse_acceptance_proof")

			if _, ok := verify.CriterionExecutor(method); ok {
				// Discharged by the passing command, and labelled as the
				// task-level proof it is.
				if len(unsupported)+len(unproven) != 0 {
					t.Fatalf("a passing command did not discharge %s: %+v", method, gaps)
				}
				if len(coarse) != 1 || coarse[0].Blocking {
					t.Fatalf("want one non-blocking coarse_acceptance_proof, got %+v", coarse)
				}
				if status, _ := verify.StatusFromGaps(gaps, meta.GateMode); status != "verified" {
					t.Errorf("status = %s: %+v", status, gaps)
				}
				return
			}
			// No executor: a blocking gap that names the method, whatever
			// the commands said.
			if len(unsupported) != 1 || !unsupported[0].Blocking {
				t.Fatalf("method %s has no executor and produced no blocking unsupported gap: %+v", method, gaps)
			}
			g := unsupported[0]
			if g.AcceptanceCriterionID == nil || *g.AcceptanceCriterionID != "AC-1" ||
				g.ExpectedVerificationMethod == nil || *g.ExpectedVerificationMethod != string(method) ||
				g.RequirementID == nil || *g.RequirementID != "REQ-1" {
				t.Errorf("gap does not name the criterion, requirement and method: %+v", g)
			}
			for _, mode := range []string{"enforce", "advisory"} {
				if status, _ := verify.StatusFromGaps(gaps, mode); status != "blocked" {
					t.Errorf("%s: an undischargeable criterion verified as %s", mode, status)
				}
			}
		})
	}
}

// manual is the method with no executor today. Pinned by name so the table
// test above cannot pass vacuously by every method having one.
func TestManualHasNoExecutor(t *testing.T) {
	if _, ok := verify.CriterionExecutor(dc.VerifyManual); ok {
		t.Fatal("manual has an executor; update this test with what discharges it")
	}
}

func TestACommandMethodIsUnprovenWhenCommandsFailOrAreAbsent(t *testing.T) {
	for _, method := range []dc.VerificationMethod{dc.VerifyUnitTest, dc.VerifyIntegrationTest, dc.VerifyStaticCheck} {
		for name, in := range map[string]verify.Input{
			"failed":  criterionInput(method, true, 1, "go test ./..."),
			"no-cmds": criterionInput(method, true, 0),
		} {
			gaps, _ := verify.Run(context.Background(), in)
			unproven := gapsOfType(gaps, "acceptance_criteria_unproven")
			if len(unproven) != 1 || !unproven[0].Blocking {
				t.Fatalf("%s/%s: want one blocking acceptance_criteria_unproven, got %+v", method, name, gaps)
			}
			if got := unproven[0].ExpectedVerificationMethod; got == nil || *got != string(method) {
				t.Errorf("%s/%s: expected_verification_method = %v", method, name, got)
			}
			if len(gapsOfType(gaps, "coarse_acceptance_proof")) != 0 {
				t.Errorf("%s/%s: an unproven criterion was also reported as proven", method, name)
			}
		}
	}
}

// required:false is DevCouncil's "nice to have". It is still reported, and it
// does not block.
func TestAnOptionalCriterionIsReportedButDoesNotBlock(t *testing.T) {
	for _, in := range []verify.Input{
		criterionInput(dc.VerifyManual, false, 0, "go test ./..."),
		criterionInput(dc.VerifyUnitTest, false, 1, "go test ./..."),
	} {
		gaps, _ := verify.Run(context.Background(), in)
		found := append(gapsOfType(gaps, "unsupported_verification_method"),
			gapsOfType(gaps, "acceptance_criteria_unproven")...)
		if len(found) != 1 || found[0].Blocking {
			t.Errorf("want one non-blocking criterion gap, got %+v", found)
		}
	}
}

func TestACriterionNoLinkedRequirementDefinesBlocks(t *testing.T) {
	in := criterionInput(dc.VerifyUnitTest, true, 0, "go test ./...")
	in.Task.AcceptanceCriterionIDs = []string{"AC-1", "AC-9"}
	in.Requirements.Missing = []string{"REQ-GONE"}
	gaps, _ := verify.Run(context.Background(), in)
	unproven := gapsOfType(gaps, "acceptance_criteria_unproven")
	if len(unproven) != 1 || !unproven[0].Blocking ||
		unproven[0].AcceptanceCriterionID == nil || *unproven[0].AcceptanceCriterionID != "AC-9" {
		t.Fatalf("want one blocking gap for AC-9, got %+v", unproven)
	}
	if !strings.Contains(strings.Join(unproven[0].Evidence, " "), "REQ-GONE") {
		t.Errorf("evidence should name the linked requirement no row defines: %v", unproven[0].Evidence)
	}
}

func TestUnreadableRequirementsBlockEveryCriterion(t *testing.T) {
	in := criterionInput(dc.VerifyUnitTest, true, 0, "go test ./...")
	in.Requirements = store.LinkedRequirements{}
	in.RequirementsErr = errors.New("store: requirement REQ-1: bad method")
	gaps, _ := verify.Run(context.Background(), in)
	unproven := gapsOfType(gaps, "acceptance_criteria_unproven")
	if len(unproven) != 1 || !unproven[0].Blocking {
		t.Fatalf("want one blocking gap, got %+v", gaps)
	}
	if !strings.Contains(strings.Join(unproven[0].Evidence, " "), "bad method") {
		t.Errorf("evidence should carry the read error: %v", unproven[0].Evidence)
	}
}

// A task that claims no criteria is unaffected: no gaps, and no store read.
func TestATaskWithNoCriteriaGetsNoCriterionGaps(t *testing.T) {
	in := criterionInput(dc.VerifyManual, true, 0, "go test ./...")
	in.Task.AcceptanceCriterionIDs = nil
	gaps, _ := verify.Run(context.Background(), in)
	for _, kind := range []string{"unsupported_verification_method", "acceptance_criteria_unproven", "coarse_acceptance_proof"} {
		if n := len(gapsOfType(gaps, kind)); n != 0 {
			t.Errorf("%d %s gaps for a task with no criteria", n, kind)
		}
	}
}
