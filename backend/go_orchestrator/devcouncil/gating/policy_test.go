package gating_test

import (
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/gating"
)

func TestHardSafety(t *testing.T) {
	if !gating.IsHardSafetyGap("security_risk") {
		t.Fatal("security_risk")
	}
	if gating.MayDemote("security_risk", true) {
		t.Fatal("must not demote security_risk")
	}
	if !gating.MayDemote("coarse_acceptance_proof", true) {
		t.Fatal("soft gaps may demote")
	}
	for _, kind := range []string{"acceptance_criteria_unproven", "llm_review_unproven"} {
		if !gating.IsHardSafetyGap(kind) {
			t.Fatalf("%s is not hard-safety", kind)
		}
		if gating.MayDemote(kind, true) {
			t.Fatalf("must not demote %s", kind)
		}
	}
}
