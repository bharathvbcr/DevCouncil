package dc

import (
	"sort"
	"strconv"
)

// Gap types emitted when verification configuration files cannot be read.
const (
	GateConfigInvalid      = "gate_config_invalid"
	VerifyEnvConfigInvalid = "verify_env_config_invalid"
)

var categoryByGapType = map[string]string{
	"orphan_diff":                     "scope",
	"planned_file_not_changed":        "scope",
	"dependency_risk":                 "scope",
	"test_failed":                     "fix_code",
	"invalid_verification_command":    "fix_verification",
	"acceptance_criteria_unproven":    "add_test",
	"diff_not_exercised":              "add_test",
	"missing_test":                    "add_test",
	"security_risk":                   "security",
	"architecture_drift":              "review",
	"assumption_violated":             "review",
	"migration_gap":                   "fix_code",
	"requirement_not_planned":         "plan",
	"task_not_implemented":            "plan",
	"stub_detected":                   "fix_code",
	"stub_declared":                   "review",
	"assert_free_test":                "add_test",
	"skipped_test":                    "add_test",
	"suspicious_effort":               "review",
	"coarse_acceptance_proof":         "add_test",
	"unwired_file":                    "fix_code",
	"dead_symbol":                     "fix_code",
	"stranded_code":                   "fix_code",
	"resolution_regression":           "fix_code",
	"stale_map":                       "refresh_map",
	"skipped_verification_command":    "fix_verification",
	"architecture_check_unavailable":  "review",
	"rigor_check_unavailable":         "fix_verification",
	"unsupported_verification_method": "fix_verification",
	"llm_review_unproven":             "review",
	"llm_review_findings":             "review",
	"runner_hijack":                   "security",
	"lean_tcb":                        "security",
	"unattested_test_run":             "fix_verification",
	"oracle_tampering":                "security",
	GateConfigInvalid:                 "fix_config",
	VerifyEnvConfigInvalid:            "fix_config",
}

// GapTypeForRigorGate maps a dcverify finding gate name to its canonical
// verification gap type. Unrecognized gates map to "rigor_check_unavailable".
func GapTypeForRigorGate(gate string) string {
	switch gate {
	case "secret_scan":
		return "security_risk"
	case "stub_detection":
		return "stub_detected"
	case "stub_allowed":
		return "stub_declared"
	case "assert_free_test":
		return "assert_free_test"
	case "skipped_test":
		return "skipped_test"
	case "runner_hijack":
		return "runner_hijack"
	case "lean_tcb":
		return "lean_tcb"
	default:
		return "rigor_check_unavailable"
	}
}

// CategoryForGapType returns the machine-routable repair category for a gap
// type, defaulting to "review" when the gap type is unrecognized.
func CategoryForGapType(gapType string) string {
	if cat, ok := categoryByGapType[gapType]; ok {
		return cat
	}
	return "review"
}

// KnownGapTypes lists every gap type routed by CategoryForGapType, sorted.
func KnownGapTypes() []string {
	out := make([]string, 0, len(categoryByGapType))
	for gapType := range categoryByGapType {
		out = append(out, gapType)
	}
	sort.Strings(out)
	return out
}

// RigorActionText returns the canonical repair instruction for a rigor gate
// finding, falling back to fallbackMessage when the gate has no specific copy.
func RigorActionText(gate, path string, line int, fallbackMessage string) string {
	target := "the affected file"
	if path != "" {
		target = path
	}
	loc := target
	if line > 0 {
		loc += ":" + strconv.Itoa(line)
	}
	switch gate {
	case "secret_scan":
		return "Remove the detected secret/finding from the diff and rotate any exposed credential."
	case "stub_detection":
		return "Replace the stub/placeholder at " + loc + " with a real implementation, then re-verify. " +
			"Do not mark work complete while placeholders remain."
	case "stub_allowed":
		return "Review the intentional stub declared at " + loc + "; replace it before marking done."
	case "assert_free_test":
		return "Make the added test assert on the behaviour it is named for, or call a helper that does."
	case "skipped_test":
		return "Remove the unconditional skip, or make the skip conditional on what the test needs."
	case "runner_hijack":
		return "Remove the test-runner hijack in " + target + " so the real test harness executes and reports failures honestly."
	case "lean_tcb":
		return "Replace the Lean 4 TCB soundness hole, metaprogramming escape, or non-standard axiom/extern hook in " + target + " with a kernel- and lean4checker-verified proof, then re-verify."
	default:
		return fallbackMessage
	}
}
