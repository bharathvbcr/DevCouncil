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
}
