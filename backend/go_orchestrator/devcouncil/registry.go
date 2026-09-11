package devcouncil

import (
	"context"
	"encoding/json"
	"fmt"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/gate"
)

// ToolBehaviour is (readOnly, destructive, idempotent, openWorld).
type ToolBehaviour struct {
	ReadOnly    bool
	Destructive bool
	Idempotent  bool
	OpenWorld   bool
}

// ToolSpec is one advertised MCP tool.
type ToolSpec struct {
	Name        string
	Description string
	InputSchema json.RawMessage
	Behaviour   ToolBehaviour
}

// Annotations maps behaviour into the MCP ToolAnnotations shape.
func (b ToolBehaviour) Annotations() map[string]any {
	return map[string]any{
		"readOnlyHint":    b.ReadOnly,
		"destructiveHint": b.Destructive,
		"idempotentHint":  b.Idempotent,
		"openWorldHint":   b.OpenWorld,
	}
}

var ro = ToolBehaviour{ReadOnly: true, Destructive: false, Idempotent: true, OpenWorld: false}

// Registry is the Phase-4 MCP tool surface. It does not import Manvi packages.
type Registry struct {
	Root  string
	Store *store.Client
	Gate  *gate.Gate
	Lease *LeaseService
}

// NewRegistry builds the MCP registry. Store/Gate may be nil when the project
// is not initialized; tools then fail closed with not_initialized.
func NewRegistry(root string, storeClient *store.Client, g *gate.Gate) *Registry {
	lease := &LeaseService{Store: storeClient}
	return &Registry{
		Root:  root,
		Store: storeClient,
		Gate:  g,
		Lease: lease,
	}
}

// Specs returns every advertised tool. tools/list is generated from this so
// schema drift between list and call is impossible.
func (r *Registry) Specs() []ToolSpec {
	return []ToolSpec{
		{
			Name:        "devcouncil_get_diff",
			Description: "Return the working-tree (or staged) git diff, optionally scoped to a task's planned files or explicit paths.",
			InputSchema: rawSchema(`{"type":"object","properties":{"task_id":{"type":"string"},"paths":{"type":"array","items":{"type":"string"}},"staged":{"type":"boolean"}},"additionalProperties":false}`),
			Behaviour:   ro,
		},
		{
			Name:        "devcouncil_checkout_task",
			Description: "Acquire a lease on a task and return its scope, lease token, and allowed next tools.",
			InputSchema: rawSchema(`{"type":"object","properties":{"task_id":{"type":"string"},"client_id":{"type":"string"},"agent":{"type":"string"},"force":{"type":"boolean"}},"required":["task_id","client_id"],"additionalProperties":false}`),
			Behaviour:   ToolBehaviour{ReadOnly: false, Destructive: false, Idempotent: false, OpenWorld: false},
		},
		{
			Name:        "devcouncil_renew_lease",
			Description: "Extend an active lease before its TTL expires.",
			InputSchema: rawSchema(`{"type":"object","properties":{"task_id":{"type":"string"},"lease_token":{"type":"string"}},"required":["task_id","lease_token"],"additionalProperties":false}`),
			Behaviour:   ToolBehaviour{ReadOnly: false, Destructive: false, Idempotent: false, OpenWorld: false},
		},
		{
			Name:        "devcouncil_release_task",
			Description: "Release a held lease so another agent can check the task out.",
			InputSchema: rawSchema(`{"type":"object","properties":{"task_id":{"type":"string"},"lease_token":{"type":"string"}},"required":["task_id","lease_token"],"additionalProperties":false}`),
			Behaviour:   ToolBehaviour{ReadOnly: false, Destructive: false, Idempotent: true, OpenWorld: false},
		},
		{
			Name:        "devcouncil_next_task",
			Description: "Find the next ready, unheld task. Does not acquire a lease.",
			InputSchema: rawSchema(`{"type":"object","properties":{"client_id":{"type":"string"}},"additionalProperties":false}`),
			Behaviour:   ro,
		},
		{
			Name:        "devcouncil_verify_task",
			Description: "Verify a leased task. Requires a valid lease_token. Returns Report/Gap/NextAction shapes matching the Python golden fixtures.",
			InputSchema: rawSchema(`{"type":"object","properties":{"task_id":{"type":"string"},"lease_token":{"type":"string"}},"required":["task_id","lease_token"],"additionalProperties":false}`),
			Behaviour:   ToolBehaviour{ReadOnly: false, Destructive: false, Idempotent: true, OpenWorld: true},
		},
		{
			Name:        "devcouncil_get_gaps",
			Description: "Return verification gaps for a task from the latest persisted verification run.",
			InputSchema: rawSchema(`{"type":"object","properties":{"task_id":{"type":"string"}},"required":["task_id"],"additionalProperties":false}`),
			Behaviour:   ro,
		},
		{
			Name:        "devcouncil_policy_check_write",
			Description: "Ask whether a write to a path would be allowed under the current gate posture and optional task scope.",
			InputSchema: rawSchema(`{"type":"object","properties":{"path":{"type":"string"},"task_id":{"type":"string"},"operation":{"type":"string"}},"required":["path"],"additionalProperties":false}`),
			Behaviour:   ro,
		},
	}
}

// Call dispatches one tool by name.
func (r *Registry) Call(ctx context.Context, name string, args map[string]any) (any, error) {
	switch name {
	case "devcouncil_get_diff":
		return r.callGetDiff(ctx, args)
	case "devcouncil_checkout_task":
		return r.callCheckout(ctx, args), nil
	case "devcouncil_renew_lease":
		return r.callRenew(ctx, args), nil
	case "devcouncil_release_task":
		return r.callRelease(ctx, args), nil
	case "devcouncil_next_task":
		return r.callNextTask(ctx, args), nil
	case "devcouncil_verify_task":
		return r.callVerify(ctx, args), nil
	case "devcouncil_get_gaps":
		return r.callGetGaps(ctx, args), nil
	case "devcouncil_policy_check_write":
		return r.callPolicyCheck(ctx, args), nil
	default:
		return nil, fmt.Errorf("unknown tool: %s", name)
	}
}

func (r *Registry) callGetDiff(ctx context.Context, args map[string]any) (any, error) {
	taskID, _ := args["task_id"].(string)
	staged, _ := args["staged"].(bool)
	var paths []string
	if raw, ok := args["paths"].([]any); ok {
		for _, v := range raw {
			if s, ok := v.(string); ok {
				paths = append(paths, s)
			}
		}
	}
	diffArgs := GetDiffArgs{TaskID: taskID, Paths: paths, Staged: staged, DBInitialized: r.Store != nil}
	if taskID != "" {
		if r.Store == nil {
			diffArgs.DBInitialized = false
		} else {
			planned, found, err := r.Lease.PlannedPaths(ctx, taskID)
			if err != nil {
				return ErrorPayload{OK: false, Code: "store_error", Error: err.Error()}, nil
			}
			diffArgs.TaskFound = found
			diffArgs.PlannedFiles = planned
			diffArgs.DBInitialized = true
		}
	}
	return GetDiff(ctx, r.Root, diffArgs)
}

func (r *Registry) callCheckout(ctx context.Context, args map[string]any) any {
	taskID, _ := args["task_id"].(string)
	clientID, _ := args["client_id"].(string)
	agent, _ := args["agent"].(string)
	force, _ := args["force"].(bool)
	if taskID == "" || clientID == "" {
		return ErrorPayload{OK: false, Code: "missing_argument", Error: "task_id and client_id are required"}
	}
	return r.Lease.Checkout(ctx, taskID, clientID, agent, force)
}

func (r *Registry) callRenew(ctx context.Context, args map[string]any) any {
	taskID, _ := args["task_id"].(string)
	token, _ := args["lease_token"].(string)
	if taskID == "" || token == "" {
		return ErrorPayload{OK: false, Code: "missing_argument", Error: "task_id and lease_token are required"}
	}
	return r.Lease.Renew(ctx, taskID, token)
}

func (r *Registry) callRelease(ctx context.Context, args map[string]any) any {
	taskID, _ := args["task_id"].(string)
	token, _ := args["lease_token"].(string)
	if taskID == "" || token == "" {
		return ErrorPayload{OK: false, Code: "missing_argument", Error: "task_id and lease_token are required"}
	}
	return r.Lease.Release(ctx, taskID, token)
}

func (r *Registry) callNextTask(ctx context.Context, args map[string]any) any {
	_ = args
	if r.Store == nil {
		return map[string]any{"ok": false, "error": "DevCouncil not initialized in this directory.", "code": "not_initialized"}
	}
	ids, err := r.Store.ReadyTasks(ctx)
	if err != nil {
		return map[string]any{"ok": false, "error": err.Error(), "code": "store_error"}
	}
	if len(ids) == 0 {
		return map[string]any{"ok": true, "task": nil}
	}
	task, err := r.Store.Task(ctx, ids[0])
	if err != nil {
		return map[string]any{"ok": false, "error": err.Error(), "code": "store_error"}
	}
	return map[string]any{"ok": true, "task_id": ids[0], "task": task}
}

func (r *Registry) callVerify(ctx context.Context, args map[string]any) any {
	taskID, _ := args["task_id"].(string)
	token, _ := args["lease_token"].(string)
	if taskID == "" || token == "" {
		return ErrorPayload{OK: false, Code: "missing_argument", Error: "task_id and lease_token are required"}
	}
	if refusal := r.Lease.RequireLease(ctx, taskID, token); refusal != nil {
		return refusal
	}
	if r.Store == nil {
		return ErrorPayload{OK: false, Code: "not_initialized", Error: "DevCouncil state is unavailable in this directory."}
	}
	gateMode := "enforce"
	if r.Lease != nil && r.Lease.GateMode != "" {
		gateMode = r.Lease.GateMode
	}
	result, _, err := verify.VerifyTask(ctx, r.Root, r.Store, taskID, gateMode, "local")
	if err != nil {
		return ErrorPayload{OK: false, Code: "verify_error", Error: err.Error()}
	}
	return result
}

func (r *Registry) callGetGaps(ctx context.Context, args map[string]any) any {
	taskID, _ := args["task_id"].(string)
	if taskID == "" {
		return ErrorPayload{OK: false, Code: "missing_argument", Error: "task_id is required"}
	}
	if r.Store == nil {
		return ErrorPayload{OK: false, Code: "not_initialized", Error: "DevCouncil state is unavailable in this directory."}
	}
	rows, truncated, err := r.Store.Gaps(ctx, taskID)
	if err != nil {
		return ErrorPayload{OK: false, Code: "store_error", Error: err.Error()}
	}
	blocking := 0
	gaps := make([]map[string]any, 0, len(rows))
	for _, row := range rows {
		if row.Blocking {
			blocking++
		}
		gaps = append(gaps, map[string]any{
			"id":              row.ID,
			"severity":        row.Severity,
			"gap_type":        row.GapType,
			"task_id":         row.TaskID,
			"description":     row.Description,
			"recommended_fix": row.RecommendedFix,
			"blocking":        row.Blocking,
			"evidence_json":   row.EvidenceJSON,
		})
	}
	return map[string]any{
		"ok":        true,
		"task_id":   taskID,
		"gaps":      gaps,
		"blocking":  blocking,
		"truncated": truncated,
	}
}

func (r *Registry) callPolicyCheck(ctx context.Context, args map[string]any) any {
	_ = ctx
	path, _ := args["path"].(string)
	if path == "" {
		return ErrorPayload{OK: false, Code: "missing_argument", Error: "path is required"}
	}
	if r.Gate == nil {
		return map[string]any{"ok": true, "path": path, "allowed": true, "note": "no gate configured"}
	}
	// Without a task, EvaluateWrite still answers secret/restricted rules.
	d, err := r.Gate.EvaluateWrite(path, nil, dc.OpModify)
	if err != nil {
		return map[string]any{"ok": false, "error": err.Error(), "code": "policy_error"}
	}
	return map[string]any{
		"ok":       true,
		"path":     path,
		"allowed":  !d.Blocked(),
		"decision": d,
	}
}

func rawSchema(s string) json.RawMessage { return json.RawMessage(s) }
