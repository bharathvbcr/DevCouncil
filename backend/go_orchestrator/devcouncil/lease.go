package devcouncil

import (
	"context"
	"errors"
	"fmt"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/gatescfg"
)

const DefaultLeaseTTL = 15 * time.Minute

// AllowedNextTools names the tools an agent may call next for a task in the
// given status. A task that is already checked out may reach for any served
// tool; one that is not must check out first.
//
// The list is derived from the registry rather than spelled again here. It used
// to mirror Python integrations/mcp/util.allowed_next_tools, and it still named
// that host's surface — devcouncil_read_file, devcouncil_get_evidence,
// devcouncil_run_command, devcouncil_apply_patch, devcouncil_write_file,
// devcouncil_update_task_scope — so six of the eight tools handed to an agent on
// every successful checkout were served by nothing. That is the same
// GAP-P7-NEXT-TOOLS-DRIFT the verify path was corrected for; this path kept its
// copy because, unlike verify's, nothing held it to the registry.
//
// Which eight is not an arbitrary choice restated here: it is the set
// ServedToolNames reports, on the rationale already written down in
// allowed_next_tools_test.go — every tool this host serves is one an agent may
// legitimately reach for, so withholding one would be a restriction the next
// person to add a tool has to rediscover.
func AllowedNextTools(status string, hasBlockingGaps bool) []string {
	_ = hasBlockingGaps
	switch status {
	case "planned", "in_progress", "blocked", "verified", "done":
		return ServedToolNames()
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
		return gatescfg.ModeOff
	}
	mode, _ := gatescfg.Normalize(s.GateMode)
	return mode
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
	// Named before it is taken, so a forced checkout can say whose lease it
	// revoked. Informational: the acquire itself is the authority.
	var revoked string
	if force {
		if prior := s.activeLease(ctx, taskID); prior != nil {
			revoked = prior.Owner
		}
	}
	lease, err := s.Store.Acquire(ctx, store.AcquireRequest{
		TaskID: taskID, Owner: owner, Agent: agent, ClientID: clientID,
		TTL: s.ttl(), Force: force,
	})
	var conflict *store.Conflict
	if errors.As(err, &conflict) {
		// The same client asking again holds the lease already: hand it back
		// rather than lock the holder out for the whole TTL (its own token is
		// the thing it lost, usually to a restarted session).
		if held := s.heldBy(ctx, taskID, clientID); held != nil {
			return s.checkoutPayload(taskID, task, held)
		}
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

	payload := s.checkoutPayload(taskID, task, lease)
	if revoked != "" {
		payload["revoked_lease_owner"] = revoked
	}
	return payload
}

// activeLease returns the live lease on a task, or nil when there is none or
// the store could not say.
func (s *LeaseService) activeLease(ctx context.Context, taskID string) *store.Lease {
	leases, err := s.Store.ActiveLeases(ctx)
	if err != nil {
		return nil
	}
	for i := range leases {
		if leases[i].TaskID == taskID {
			return &leases[i]
		}
	}
	return nil
}

// heldBy returns the live lease on a task when it belongs to clientID.
func (s *LeaseService) heldBy(ctx context.Context, taskID, clientID string) *store.Lease {
	if clientID == "" {
		return nil
	}
	if lease := s.activeLease(ctx, taskID); lease != nil && lease.ClientID == clientID {
		return lease
	}
	return nil
}

// checkoutPayload is the success reply for a lease held on a task. The tool
// list is the served set whatever the task's status: a lease was just taken,
// and listing only devcouncil_checkout_task for a `ready` task sent the agent
// that held the lease back to check out again.
func (s *LeaseService) checkoutPayload(taskID string, task *store.Task, lease *store.Lease) map[string]any {
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
		"allowed_next_tools": ServedToolNames(),
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
	if !released {
		// A token that does not hold the lease released nothing. Reporting that
		// as ok:true sent callers on believing the task was free.
		return map[string]any{
			"ok": false, "released": false, "task_id": taskID, "code": "lease_not_released",
			"error": "That token does not hold the lease on this task, so nothing was released.",
		}
	}
	return map[string]any{"ok": true, "released": true, "task_id": taskID}
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
