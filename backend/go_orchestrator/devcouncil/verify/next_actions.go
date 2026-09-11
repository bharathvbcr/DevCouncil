package verify

import (
	"sort"
	"strings"
)

var categoryByGapType = map[string]string{
	"orphan_diff":                    "scope",
	"planned_file_not_changed":       "scope",
	"dependency_risk":                "scope",
	"test_failed":                    "fix_code",
	"invalid_verification_command":   "fix_verification",
	"acceptance_criteria_unproven":   "add_test",
	"diff_not_exercised":             "add_test",
	"missing_test":                   "add_test",
	"security_risk":                  "security",
	"architecture_drift":             "review",
	"assumption_violated":            "review",
	"migration_gap":                  "fix_code",
	"requirement_not_planned":        "plan",
	"task_not_implemented":           "plan",
	"stub_detected":                  "fix_code",
	"stub_declared":                  "review",
	"suspicious_effort":              "review",
	"coarse_acceptance_proof":        "add_test",
	"unwired_file":                   "fix_code",
	"dead_symbol":                    "fix_code",
	"stranded_code":                  "fix_code",
	"resolution_regression":          "fix_code",
	"stale_map":                      "refresh_map",
	"skipped_verification_command":   "fix_verification",
	"architecture_check_unavailable": "review",
}

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
	switch g.GapType {
	case "orphan_diff":
		return "Revert changes to " + target + " or append it with " +
			"`dev scope update <task_id> --lease-token <token> --planned-file " + target + "`."
	case "planned_file_not_changed":
		return "Modify " + target + " as planned, or remove it from the task's planned files."
	case "dependency_risk":
		return "Justify or revert the unplanned dependency/config change in " + target + "."
	case "test_failed":
		if g.SuggestedCommand != nil && *g.SuggestedCommand != "" {
			return "Fix the failing check, then re-run: " + *g.SuggestedCommand
		}
		return "Fix the failing verification check, then re-verify."
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
		return "Remove the detected secret/finding from the diff and rotate any exposed credential."
	case "architecture_drift":
		return "Address the flagged change or resolve the open critique card, then re-verify."
	case "stub_detected":
		loc := target
		if g.Line != nil {
			loc += ":" + itoa(*g.Line)
		}
		return "Replace the stub/placeholder at " + loc + " with a real implementation, then re-verify. " +
			"Do not mark work complete while placeholders remain."
	case "stub_declared":
		loc := target
		if g.Line != nil {
			loc += ":" + itoa(*g.Line)
		}
		return "Review the intentional stub declared at " + loc + "; replace it before marking done."
	case "suspicious_effort":
		return "The diff looks too small or superficial for the planned scope. Complete the " +
			"planned work (or restore removed tests), then re-verify."
	case "coarse_acceptance_proof":
		return "Acceptance criteria were proven only by a coarse passing command, not a " +
			"per-criterion check. Add a verification command or test that exercises each " +
			"listed criterion specifically, then re-verify."
	case "unwired_file":
		return "Import or register `" + target + "` from its intended non-test caller " +
			"(append the caller with `dev scope update <task_id> --lease-token <token> " +
			"--planned-file <caller>` if needed), or delete the unused file."
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
			"(use `dev scope update ... --planned-file <caller>` if the caller is " +
			"out of scope), or remove it."
	case "stranded_code":
		return "Restore the import/call that kept `" + target + "` live, or delete the " +
			"stranded module if it is intentionally unused."
	case "stale_map":
		return "Run `dev map` to regenerate the repository map, then re-verify."
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

// NextActionFor builds one NextAction from a Gap.
func NextActionFor(g Gap) NextAction {
	file := deriveFile(g)
	cat := categoryByGapType[g.GapType]
	if cat == "" {
		cat = "review"
	}
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
