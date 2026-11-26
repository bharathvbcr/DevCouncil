package verify

import (
	"sort"
	"strings"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/dcverify"
)

func looksLikePath(value string) bool {
	value = strings.TrimSpace(value)
	if value == "" || strings.ContainsAny(value, " \n") {
		return false
	}
	return strings.Contains(value, "/") || strings.Contains(value, ".")
}

func deriveFile(g Gap) *string {
	if g.File != nil && *g.File != "" {
		return g.File
	}
	for _, item := range g.Evidence {
		if looksLikePath(item) {
			return filePtr(item)
		}
	}
	return nil
}

func actionText(g Gap, file *string) string {
	target := "the affected file"
	if file != nil && *file != "" {
		target = *file
	}
	line := -1
	if g.Line != nil {
		line = *g.Line
	}
	switch g.GapType {
	case "orphan_diff":
		return "Revert changes to " + target + ", or ask the task's owner to add it to the " +
			"task's planned files."
	case "planned_file_not_changed":
		return "Modify " + target + " as planned, or remove it from the task's planned files."
	case "dependency_risk":
		return "Justify or revert the unplanned dependency/config change in " + target + "."
	case "test_failed":
		if g.SuggestedCommand != nil && *g.SuggestedCommand != "" {
			return "Fix the failing check, then re-run: " + *g.SuggestedCommand
		}
		return "Fix the failing verification check, then re-verify."
	case GateConfigInvalid:
		return "Fix the gate config named in the gap (gates.mode is off, advisory or enforce), then re-verify."
	case VerifyEnvConfigInvalid:
		return "Write verify.env_allow in .devcouncil/config.yaml as an inline list, [NAME, NAME], then re-verify."
	case "invalid_verification_command":
		return "Replace the unrunnable verification command with a single runnable command, then re-verify."
	case "diff_not_exercised":
		loc := ""
		if file != nil {
			loc = " (" + target
			if g.Line != nil {
				loc += ":" + itoa(*g.Line)
			}
			loc += ")"
		}
		return "Add or extend a test that executes the changed lines" + loc + ", then re-verify."
	case "acceptance_criteria_unproven", "missing_test":
		return "Provide a passing verification command that proves this acceptance criterion."
	case "security_risk":
		return dc.RigorActionText(dcverify.GateSecretScan, target, line, g.RecommendedFix)
	case "architecture_drift":
		return "Address the flagged change or resolve the open critique card, then re-verify."
	case "stub_detected":
		return dc.RigorActionText(dcverify.GateStubDetection, target, line, g.RecommendedFix)
	case "stub_declared":
		return dc.RigorActionText(dcverify.GateStubAllowed, target, line, g.RecommendedFix)
	case "assert_free_test":
		if g.RecommendedFix != "" {
			return g.RecommendedFix
		}
		return dc.RigorActionText(dcverify.GateAssertFreeTest, target, line, g.RecommendedFix)
	case "skipped_test":
		if g.RecommendedFix != "" {
			return g.RecommendedFix
		}
		return dc.RigorActionText(dcverify.GateSkippedTest, target, line, g.RecommendedFix)
	case "suspicious_effort":
		return "The diff looks too small or superficial for the planned scope. Complete the " +
			"planned work (or restore removed tests), then re-verify."
	case "coarse_acceptance_proof":
		return "Acceptance criteria were proven only by a coarse passing command, not a " +
			"per-criterion check. Add a verification command or test that exercises each " +
			"listed criterion specifically, then re-verify."
	case "unwired_file":
		return "Import or register `" + target + "` from its intended non-test caller " +
			"(if that caller is outside the planned files, ask the task's owner to add it), " +
			"or delete the unused file."
	case "dead_symbol":
		loc := target
		if g.Line != nil {
			loc += ":" + itoa(*g.Line)
		}
		symbol := "the new symbol"
		for _, item := range g.Evidence {
			if strings.HasPrefix(item, "symbol:") {
				symbol = strings.TrimSpace(strings.TrimPrefix(item, "symbol:"))
				break
			}
		}
		return "Call or register `" + symbol + "` at " + loc + " from the code that needs it " +
			"(if that caller is outside the planned files, ask the task's owner to add it), " +
			"or remove it."
	case "stranded_code":
		return "Restore the import/call that kept `" + target + "` live, or delete the " +
			"stranded module if it is intentionally unused."
	case "unsupported_verification_method":
		criterion, method := "this acceptance criterion", "its verification method"
		if g.AcceptanceCriterionID != nil {
			criterion = *g.AcceptanceCriterionID
		}
		if g.ExpectedVerificationMethod != nil {
			method = *g.ExpectedVerificationMethod
		}
		return "Verify cannot check " + criterion + " (" + method + "). Have a person confirm it and " +
			"re-plan it with unit_test, integration_test or static_check, or mark it required: false."
	case "llm_review_unproven":
		criterion := "this acceptance criterion"
		if g.AcceptanceCriterionID != nil {
			criterion = *g.AcceptanceCriterionID
		}
		return criterion + " is proved by an independent review of exactly this content, which " +
			"you cannot record yourself. Finish the change, then ask for the review: the harness " +
			"that runs the reviewers records it (`devcouncil review record`, refused in agent " +
			"shells). If the reasons name open blocker findings, fix them first."
	case "llm_review_findings":
		return "The review that discharged this criterion left major findings open or set " +
			"findings aside; read them in the evidence and fix or answer each before the task is " +
			"accepted."
	case "stale_map":
		return "Run `dev map` to regenerate the repository map, then re-verify."
	case "runner_hijack":
		return dc.RigorActionText(dcverify.GateRunnerHijack, target, line, g.RecommendedFix)
	case "lean_tcb":
		return dc.RigorActionText(dcverify.GateLeanTCB, target, line, g.RecommendedFix)
	case "unattested_test_run":
		if g.SuggestedCommand != nil && *g.SuggestedCommand != "" {
			return "Ensure the test command executes at least one attested test and emits its runner summary without early process exit: " + *g.SuggestedCommand
		}
		return "Ensure the verification test command executes at least one attested test and emits its runner summary."
	case "oracle_tampering":
		return "Revert unauthorized edits to held-out oracle or test-closure files in " + target + ", then re-verify."
	default:
		return g.RecommendedFix
	}
}

func missingEvidence(g Gap) *string {
	if g.GapType == "acceptance_criteria_unproven" {
		parts := make([]string, 0, 4)
		if g.AcceptanceCriterionID != nil && *g.AcceptanceCriterionID != "" {
			parts = append(parts, "No passing evidence for acceptance criterion "+*g.AcceptanceCriterionID)
		} else {
			parts = append(parts, "No passing acceptance evidence")
		}
		if g.ExpectedVerificationMethod != nil && *g.ExpectedVerificationMethod != "" {
			parts = append(parts, "expected verification method: "+*g.ExpectedVerificationMethod)
		}
		if g.SuggestedCommand != nil && *g.SuggestedCommand != "" {
			parts = append(parts, "run/repair check: "+*g.SuggestedCommand)
		} else if g.File != nil {
			loc := *g.File
			if g.Line != nil {
				loc += ":" + itoa(*g.Line)
			}
			parts = append(parts, "uncovered: "+loc)
		}
		s := strings.Join(parts, "; ")
		return &s
	}
	if g.GapType == "diff_not_exercised" || g.GapType == "missing_test" {
		return strPtr(g.Description)
	}
	return nil
}

// GapTypes lists every gap type this package routes to a category, sorted.
//
// It exists so the host can read every action text NextActionFor writes and
// hold the commands they name to its dispatch table: those texts once told
// agents to run `dev scope update`, which no host dispatches.
func GapTypes() []string {
	return dc.KnownGapTypes()
}

// NextActionFor builds one NextAction from a Gap.
func NextActionFor(g Gap) NextAction {
	file := deriveFile(g)
	cat := dc.CategoryForGapType(g.GapType)
	ev := g.Evidence
	if ev == nil {
		ev = []string{}
	}
	return NextAction{
		GapID:                      g.ID,
		GapType:                    g.GapType,
		Category:                   cat,
		Severity:                   g.Severity,
		Blocking:                   g.Blocking,
		Action:                     actionText(g, file),
		File:                       file,
		Line:                       g.Line,
		AcceptanceCriterionID:      g.AcceptanceCriterionID,
		ExpectedVerificationMethod: g.ExpectedVerificationMethod,
		MissingEvidence:            missingEvidence(g),
		SuggestedCommand:           g.SuggestedCommand,
		Evidence:                   ev,
		StdoutPath:                 g.StdoutPath,
		StderrPath:                 g.StderrPath,
	}
}

// BuildNextActions returns next actions, optionally blocking-only.
func BuildNextActions(gaps []Gap, blockingOnly bool) []NextAction {
	selected := make([]Gap, 0, len(gaps))
	for _, g := range gaps {
		if blockingOnly && !g.Blocking {
			continue
		}
		selected = append(selected, g)
	}
	sort.SliceStable(selected, func(i, j int) bool {
		a, b := selected[i], selected[j]
		if a.Blocking != b.Blocking {
			return a.Blocking
		}
		return severityRank[a.Severity] < severityRank[b.Severity]
	})
	out := make([]NextAction, 0, len(selected))
	for _, g := range selected {
		out = append(out, NextActionFor(g))
	}
	return out
}

// SplitNextActions returns (blocking, advisory).
func SplitNextActions(gaps []Gap) (blocking, advisory []NextAction) {
	all := BuildNextActions(gaps, false)
	for _, a := range all {
		if a.Blocking {
			blocking = append(blocking, a)
		} else {
			advisory = append(advisory, a)
		}
	}
	if blocking == nil {
		blocking = []NextAction{}
	}
	if advisory == nil {
		advisory = []NextAction{}
	}
	return blocking, advisory
}
