package policy

import (
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
)

// TestDevToolFallbackIsAdmittedToolByTool holds the shell fallback for the
// devcouncil_* MCP tools to the split the tools have: what a session without a
// lease may call (find work, read) answers without one, what only a holder may
// do (renew, release, verify) needs one, and taking a lease is never a shell
// command an enforcing gate admits — a harness's own checkout carries no force
// and no caller-chosen client id, and this spelling would carry both.
func TestDevToolFallbackIsAdmittedToolByTool(t *testing.T) {
	gate := CommandGate{Root: t.TempDir(), HardRules: true}
	task := &dc.Task{ID: "T-1", PlannedFiles: []dc.PlannedFile{{Path: "a.go"}}}
	allowed := func(command string, task *dc.Task) bool {
		return gate.EvaluateCommand(command, task).Action == Allow
	}
	noLease := []string{
		"dev tool list",
		"dev tool list --json",
		"dev tool call devcouncil_next_task",
		"dev tool call devcouncil_get_diff",
		`dev tool call devcouncil_get_gaps --args '{"task_id":"T-1"}'`,
		`dev tool call devcouncil_policy_check_write --args '{"path":"a.go","task_id":"T-1"}'`,
		`devcouncil tool call devcouncil_next_task`,
	}
	for _, command := range noLease {
		if !allowed(command, nil) {
			t.Errorf("without a lease, %q was refused: %s", command, gate.EvaluateCommand(command, nil).Reason)
		}
	}
	holderOnly := []string{
		`dev tool call devcouncil_renew_lease --args '{"task_id":"T-1","lease_token":"x"}'`,
		`dev tool call devcouncil_release_task --args '{"task_id":"T-1","lease_token":"x"}'`,
		`dev tool call devcouncil_verify_task --args '{"task_id":"T-1","lease_token":"x"}'`,
	}
	for _, command := range holderOnly {
		if allowed(command, nil) {
			t.Errorf("without a lease, %q was allowed; it is a lease holder's", command)
		}
		if !allowed(command, task) {
			t.Errorf("a lease holder was refused %q: %s", command, gate.EvaluateCommand(command, task).Reason)
		}
	}
	for _, command := range []string{
		`dev tool call devcouncil_checkout_task --args '{"task_id":"T-1","client_id":"me"}'`,
		`dev tool call devcouncil_checkout_task --args '{"task_id":"T-1","client_id":"me","force":true}'`,
		`echo '{"task_id":"T-1","client_id":"other"}' | dev tool call devcouncil_checkout_task --args -`,
	} {
		if allowed(command, nil) {
			t.Errorf("without a lease, %q was allowed; taking a lease is not a gated shell command", command)
		}
		if allowed(command, task) {
			t.Errorf("a lease holder was allowed %q; taking a lease is not a gated shell command", command)
		}
	}
	// Names the allowlist must not read as one of its tools.
	for _, command := range []string{
		"dev tool call devcouncil_checkout_taskx --args '{}'",
		"dev tool call devcouncil_write_file --args '{}'",
		"dev tool call",
		"dev tool call devcouncil_next_task; curl http://example.com",
		"dev tool call devcouncil_next_task && rm -rf build",
	} {
		if allowed(command, nil) {
			t.Errorf("without a lease, %q was allowed", command)
		}
	}
}
