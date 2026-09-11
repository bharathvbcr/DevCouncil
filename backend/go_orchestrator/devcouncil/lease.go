package devcouncil

import (
	"context"
	"errors"
	"fmt"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
)

const DefaultLeaseTTL = 15 * time.Minute

// AllowedNextTools mirrors Python integrations/mcp/util.allowed_next_tools for
// a freshly checked-out planned task (no blocking gaps).
func AllowedNextTools(status string, hasBlockingGaps bool) []string {
	_ = hasBlockingGaps
	switch status {
	case "planned", "in_progress", "blocked", "verified", "done":
		return []string{
			"devcouncil_read_file",
			"devcouncil_get_evidence",
			"devcouncil_get_diff",
			"devcouncil_run_command",
			"devcouncil_apply_patch",
			"devcouncil_write_file",
			"devcouncil_update_task_scope",
			"devcouncil_verify_task",
		}
	default:
		return []string{"devcouncil_checkout_task"}
	}
}

// AppliedSkillNames is the Phase-4 default skill set returned on checkout.
// Full selection is Phase 5+; golden fixtures record this fixed list.
func AppliedSkillNames() []string {
	return []string{
		"core-engineering",
		"devcouncil",
		"devcouncil-hero-loop",
		"devcouncil-verification",
		"devmap",
		"devmap-debugging",
		"devmap-exploring",
		"devmap-impact",
		"devmap-refactoring",
	}
}

// LeaseService talks to dcstore for task/lease verbs the MCP surface needs.
type LeaseService struct {
	Store    *store.Client
	LeaseTTL time.Duration
	GateMode string
}

func (s *LeaseService) ttl() time.Duration {
	if s.LeaseTTL <= 0 {
		return DefaultLeaseTTL
	}
	return s.LeaseTTL
}

func (s *LeaseService) gateMode() string {
	if s.GateMode == "" {
		return "enforce"
	}
	return s.GateMode
}

// Checkout acquires a lease and returns the Python MCP checkout payload shape.
func (s *LeaseService) Checkout(ctx context.Context, taskID, clientID, agent string, force bool) map[string]any {
	if s.Store == nil {
		return map[string]any{"ok": false, "error": "DevCouncil state is unavailable in this directory.", "code": "not_initialized"}
	}
	task, err := s.Store.Task(ctx, taskID)
	if err != nil {
		return map[string]any{"ok": false, "error": err.Error(), "code": "store_error"}
	}
	if task == nil {
		return map[string]any{"ok": false, "error": fmt.Sprintf("Task %s not found.", taskID), "code": "not_found", "task_id": taskID}
	}

	owner := clientID
	if owner == "" {
		owner = "devcouncil"
	}
	lease, err := s.Store.Acquire(ctx, store.AcquireRequest{
		TaskID: taskID, Owner: owner, Agent: agent, ClientID: clientID,
		TTL: s.ttl(), Force: force,
	})
	var conflict *store.Conflict
	if errors.As(err, &conflict) {
		// Python lease_ops surfaces lease_conflict (not lease_held_by_other).
		return map[string]any{
			"ok":             false,
			"code":           "lease_conflict",
			"error":          fmt.Sprintf("Active lease already exists for task %s", taskID),
			"task_id":        taskID,
			"applied_skills": AppliedSkillNames(),
		}
	}
	if err != nil {
		return map[string]any{"ok": false, "error": err.Error(), "code": "store_error", "task_id": taskID}
	}

	planned := make([]map[string]any, 0, len(task.PlannedFiles))
	for _, pf := range task.PlannedFiles {
		entry := map[string]any{
			"path":           pf.Path,
			"allowed_change": string(pf.AllowedChange),
		}
		planned = append(planned, entry)
	}
	status := task.Status
	if status == "" {
		status = "planned"
	}
	allowedCmds := task.AllowedCommands
	if allowedCmds == nil {
		allowedCmds = []string{}
	}
	expected := task.ExpectedTests
	if expected == nil {
		expected = []string{}
	}
	return map[string]any{
		"ok":                 true,
		"task_id":            taskID,
		"lease_token":        lease.Token,
		"expires_at":         lease.ExpiresAt,
		"gate_mode":          s.gateMode(),
		"status":             status,
		"planned_files":      planned,
		"allowed_commands":   allowedCmds,
		"expected_tests":     expected,
		"allowed_next_tools": AllowedNextTools(status, false),
		"applied_skills":     AppliedSkillNames(),
		"prompt":             "",
		"semantic_context":   nil,
	}
}

// Renew extends a lease; expired returns the Python renew/verify shape.
func (s *LeaseService) Renew(ctx context.Context, taskID, token string) map[string]any {
	if s.Store == nil {
		return map[string]any{"ok": false, "error": "DevCouncil state is unavailable in this directory.", "code": "not_initialized"}
	}
	code, action, tool, err := s.Store.Diagnose(ctx, taskID, token)
	if err != nil {
		return map[string]any{"ok": false, "error": err.Error(), "code": "store_error"}
	}
	if code == dc.LeaseExpired {
		return expiredLeasePayload(taskID)
	}
	if code != dc.LeaseValid {
		return map[string]any{
			"ok": false, "code": string(code), "error": fmt.Sprintf("lease not valid: %s", code),
			"task_id": taskID, "suggested_action": action, "suggested_tool": tool,
		}
	}
	lease, err := s.Store.Renew(ctx, taskID, token, s.ttl())
	if err != nil {
		return map[string]any{"ok": false, "error": err.Error(), "code": "store_error"}
	}
	if lease == nil {
		return expiredLeasePayload(taskID)
	}
	return map[string]any{
		"ok": true, "task_id": taskID, "lease_token": lease.Token, "expires_at": lease.ExpiresAt,
	}
}

// Release ends a lease.
func (s *LeaseService) Release(ctx context.Context, taskID, token string) map[string]any {
	if s.Store == nil {
		return map[string]any{"ok": false, "error": "DevCouncil state is unavailable in this directory.", "code": "not_initialized"}
	}
	released, err := s.Store.Release(ctx, taskID, token)
	if err != nil {
		return map[string]any{"ok": false, "error": err.Error(), "code": "store_error"}
	}
	return map[string]any{"ok": true, "released": released, "task_id": taskID}
}

// RequireLease returns nil when the token is valid, or an error payload.
func (s *LeaseService) RequireLease(ctx context.Context, taskID, token string) map[string]any {
	if s.Store == nil {
		return map[string]any{"ok": false, "error": "DevCouncil state is unavailable in this directory.", "code": "not_initialized"}
	}
	code, action, tool, err := s.Store.Diagnose(ctx, taskID, token)
	if err != nil {
		return map[string]any{"ok": false, "error": err.Error(), "code": "store_error"}
	}
	if code == dc.LeaseExpired {
		return expiredLeasePayload(taskID)
	}
	if code != dc.LeaseValid {
		return map[string]any{
			"ok": false, "code": string(code),
			"error":            fmt.Sprintf("lease not valid: %s", code),
			"task_id":          taskID,
			"suggested_action": action,
			"suggested_tool":   tool,
		}
	}
	return nil
}

func expiredLeasePayload(taskID string) map[string]any {
	return map[string]any{
		"ok":               false,
		"code":             string(dc.LeaseExpired),
		"error":            "Lease TTL expired. Check out the task again with devcouncil_checkout_task.",
		"hint":             "Renew only works before TTL expiry; call checkout again after expiry.",
		"suggested_action": "checkout_again",
		"suggested_tool":   "devcouncil_checkout_task",
		"task_id":          taskID,
	}
}

// PlannedPaths returns planned file paths for a task, or ok=false details.
func (s *LeaseService) PlannedPaths(ctx context.Context, taskID string) (paths []string, found bool, err error) {
	if s.Store == nil {
		return nil, false, errors.New("not initialized")
	}
	task, err := s.Store.Task(ctx, taskID)
	if err != nil {
		return nil, false, err
	}
	if task == nil {
		return nil, false, nil
	}
	for _, pf := range task.PlannedFiles {
		paths = append(paths, pf.Path)
	}
	return paths, true, nil
}
