package verify_test

import (
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
	"testing"
)

func TestSkippedVerificationCannotReportVerified(t *testing.T) {
	for _, mode := range []string{"", "off", "no"} {
		status, passed := verify.StatusFromGaps(nil, mode)
		if status == "verified" || passed {
			t.Errorf("mode=%q: unchecked work reports status=%s passed=%v", mode, status, passed)
		}
	}
}
