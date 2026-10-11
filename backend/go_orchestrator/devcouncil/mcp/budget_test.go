package mcp

import (
	"encoding/json"
	"testing"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
)

// Every tool shared a 60 s budget, which cut a verification short at a fraction
// of verify.CommandTimeout and, with the run recorded on the expired context,
// lost it. The tool that hosts a test suite now outlives one.
func TestVerifyTaskOutlivesItsCommandsAndOtherToolsKeepTheShortBudget(t *testing.T) {
	t.Setenv(callBudgetEnv, "")
	verifyCall := json.RawMessage(`{"name":"devcouncil_verify_task","arguments":{}}`)
	if got := callBudgetFor("tools/call", verifyCall); got < verify.CommandTimeout {
		t.Errorf("verify_task budget %s is shorter than a verification command's %s", got, verify.CommandTimeout)
	}
	for _, other := range []json.RawMessage{
		json.RawMessage(`{"name":"devcouncil_get_gaps","arguments":{}}`),
		json.RawMessage(`{"name":`),
		nil,
	} {
		if got := callBudgetFor("tools/call", other); got != defaultCallBudget {
			t.Errorf("budget for %s = %s, want %s", other, got, defaultCallBudget)
		}
	}
	if got := callBudgetFor("tools/list", nil); got != defaultCallBudget {
		t.Errorf("tools/list budget = %s", got)
	}
}

func TestTheBudgetKnobOverridesEveryTool(t *testing.T) {
	t.Setenv(callBudgetEnv, "1500")
	verifyCall := json.RawMessage(`{"name":"devcouncil_verify_task"}`)
	if got := callBudgetFor("tools/call", verifyCall); got != 1500*time.Millisecond {
		t.Errorf("budget with the knob set = %s, want 1.5s", got)
	}
	t.Setenv(callBudgetEnv, "nonsense")
	if got := callBudgetFor("ping", nil); got != defaultCallBudget {
		t.Errorf("a malformed knob must fall back to the default, got %s", got)
	}
}
