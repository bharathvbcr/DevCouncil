// Package gating ports the Python gating/policy hard-safety helpers that the
// Go gate package does not already own. Write-path policy lives in
// backend/go_orchestrator/policy + gate; this package covers verification-gap
// classification used by report/task UIs.
package gating

// HardSafetyGapTypes are gap types that must never be demoted to advisory,
// matching Python gating.policy.is_hard_safety_gap.
var HardSafetyGapTypes = map[string]struct{}{
	"security_risk":                {},
	"dependency_risk":              {},
	"orphan_diff":                  {},
	"task_not_implemented":         {},
	"stub_detected":                {},
	"invalid_verification_command": {}, // advisory by default, but never silently dropped
}

// IsHardSafetyGap reports whether a gap type is in the hard-safety set.
func IsHardSafetyGap(gapType string) bool {
	_, ok := HardSafetyGapTypes[gapType]
	return ok
}

// MayDemote returns false for hard-safety types. Soft types may be demoted
// under gate_mode=warn.
func MayDemote(gapType string, blocking bool) bool {
	if !blocking {
		return false
	}
	if IsHardSafetyGap(gapType) {
		// invalid_verification_command is listed for visibility but Python
		// treats it as non-blocking; demotion does not apply.
		if gapType == "invalid_verification_command" {
			return false
		}
		return false
	}
	return true
}
