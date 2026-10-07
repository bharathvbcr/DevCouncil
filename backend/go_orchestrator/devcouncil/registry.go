package devcouncil

import (
	"context"
	"encoding/json"
	"fmt"
	"sort"
	"strings"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/gatescfg"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/stopgate"
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
	lease.GateMode = gatescfg.Load(root).VerificationMode
	return &Registry{
		Root:  root,
		Store: storeClient,
		Gate:  g,
		Lease: lease,
	}
}

// Specs returns every advertised tool. tools/list is generated from this so
// schema drift between list and call is impossible.
func (r *Registry) Specs() []ToolSpec { return toolSpecs() }

// ServedToolNames names every tool this host advertises.
//
// Anything inside this package that needs to name the served surface reads it
// from here instead of spelling a second list. A hand-written copy is precisely
// how the checkout path came to hand every agent six tool names this host does
// not have (GAP-P7-NEXT-TOOLS-DRIFT). verify.AllowedNextToolsForVerify still
// keeps a copy because package devcouncil imports verify and reading back would
// cycle — that copy is safe only because allowed_next_tools_test.go holds it to
// this list in both directions. Callers that can reach this function have no
// such excuse.
func ServedToolNames() []string {
	specs := toolSpecs()
	names := make([]string, len(specs))
	for i, spec := range specs {
		names[i] = spec.Name
	}
	return names
}

// toolSpecs is the one literal list of advertised tools.
func toolSpecs() []ToolSpec {
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
			Description: "Ask whether a write to a path would be allowed under the current gate posture. Pass task_id to judge it against that task's planned scope; without one the answer stops at task.absent after the secret, restricted and outside-root rules. operation is create, modify (the default) or delete.",
			InputSchema: rawSchema(`{"type":"object","properties":{"path":{"type":"string"},"task_id":{"type":"string"},"operation":{"type":"string","enum":["create","modify","delete"]}},"required":["path"],"additionalProperties":false}`),
			Behaviour:   ro,
		},
	}
}

// Call dispatches one tool by name.
//
// Arguments the tool's own input schema does not declare are refused first,
// with invalid_argument, before any lease, store or git is consulted. Nothing
// else enforces a schema at runtime, and an argument dropped without a word is
// a configuration the caller believes it made: `sandbox: "docker"` on
// verify_task came back as a report the caller would read as the isolated run
// it asked for, while every command ran on the host (TASK-P7-2).
func (r *Registry) Call(ctx context.Context, name string, args map[string]any) (any, error) {
	if refusal, ok := undeclaredArgument(name, args); ok {
		return refusal, nil
	}
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
	gateMode := ""
	if r.Lease != nil {
		gateMode = r.Lease.GateMode
	}
	if gateMode == "" {
		gateMode = gatescfg.Load(r.Root).VerificationMode
	}
	out := stopgate.Run(ctx, stopgate.Input{
		Root: r.Root, Store: r.Store, TaskID: taskID, GateMode: gateMode, Sandbox: verify.SandboxLocal,
	})
	if out.Decision.Skipped {
		return ErrorPayload{OK: false, Code: "verify_error", Error: out.Decision.Reason}
	}
	return out.MCP
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
	// Recurrence rides along with the gap rather than sitting behind a tool of
	// its own. "You have reported this fixed before" is only useful at the
	// moment something is deciding what to do about the gap, and a separate
	// tool is one an agent has to know to call — which the ones that most need
	// this signal are the least likely to do.
	//
	// A history read that fails is not fatal here: the gaps are the answer to
	// this call and withholding them because an annotation could not be
	// gathered would be the worse failure. The absence is reported instead, so
	// a caller can tell "no gap has ever recurred" from "recurrence was not
	// available" — the two are opposite facts and must not share a rendering.
	history := map[string]store.GapHistoryRow{}
	historyAvailable := true
	if rows, _, err := r.Store.GapHistory(ctx, taskID); err == nil {
		for _, h := range rows {
			history[h.GapID] = h
		}
	} else {
		historyAvailable = false
	}

	blocking := 0
	gaps := make([]map[string]any, 0, len(rows))
	for _, row := range rows {
		if row.Blocking {
			blocking++
		}
		gap := map[string]any{
			"id":              row.ID,
			"severity":        row.Severity,
			"gap_type":        row.GapType,
			"task_id":         row.TaskID,
			"description":     row.Description,
			"recommended_fix": row.RecommendedFix,
			"blocking":        row.Blocking,
			"evidence_json":   row.EvidenceJSON,
		}
		if h, ok := history[row.ID]; ok {
			gap["occurrences"] = h.Occurrences
			gap["resurfaces"] = h.Resurfaces
			gap["first_seen_run"] = h.FirstSeenRun
		}
		gaps = append(gaps, gap)
	}
	return map[string]any{
		"ok":       true,
		"task_id":  taskID,
		"gaps":     gaps,
		"blocking": blocking,
		// False means the recurrence annotation is missing from every gap
		// above, not that nothing has ever recurred.
		"recurrence_available": historyAvailable,
		"truncated":            truncated,
	}
}

// callPolicyCheck judges a write against the named task's scope. It used to
// read only path: task_id was dropped without a word, so the ladder stopped at
// task.absent and a caller asking "is this in my task's scope?" got an answer
// about no task at all, which it would read as the one it asked for.
//
// Call has already refused any argument the schema does not declare, of the
// wrong type, or outside its enum, so operation is one of create, modify or
// delete here. dc.OpWrite is deliberately not offered: a caller that can say
// which of create and modify it means gets the stricter answer.
func (r *Registry) callPolicyCheck(ctx context.Context, args map[string]any) any {
	path, _ := args["path"].(string)
	if path == "" {
		return ErrorPayload{OK: false, Code: "missing_argument", Error: "path is required"}
	}
	op := dc.OpModify
	if raw, given := args["operation"].(string); given {
		op = dc.Operation(raw)
	}
	taskID, taskGiven := args["task_id"].(string)
	if taskGiven && taskID == "" {
		return ErrorPayload{OK: false, Code: "invalid_argument",
			Error: "devcouncil_policy_check_write: task_id is empty; omit it to ask without a task"}
	}
	if r.Gate == nil {
		return map[string]any{"ok": true, "path": path, "allowed": true, "note": "no gate configured"}
	}
	// Without a task, EvaluateWrite still answers secret/restricted rules. With
	// one, every failure to load it is its own answer: falling through to a nil
	// task would report task.absent for a store that could not be read.
	var task *dc.Task
	if taskGiven {
		if r.Store == nil {
			return ErrorPayload{OK: false, Code: "not_initialized",
				Error: "DevCouncil state is unavailable in this directory, so task " + taskID + " could not be loaded."}
		}
		stored, err := r.Store.Task(ctx, taskID)
		if err != nil {
			return ErrorPayload{OK: false, Code: "store_error", Error: err.Error()}
		}
		if stored == nil {
			return map[string]any{"ok": false, "error": fmt.Sprintf("Task %s not found.", taskID), "code": "not_found", "task_id": taskID}
		}
		task = stored.Domain()
	}
	d, err := r.Gate.EvaluateWrite(path, task, op)
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

// argumentHints says, for an argument callers send expecting it to work, why
// it is refused rather than merely that it is.
var argumentHints = map[string]string{
	"sandbox": "verification runs on the host and the only sandbox is " + verify.SandboxLocal,
}

// undeclaredArgument refuses an argument the named tool's input schema does
// not declare, when that schema sets additionalProperties:false. The schema is
// read from toolSpecs, the same text tools/list advertises, so the check and
// the advertisement cannot disagree. Names are checked in sorted order so the
// refusal is deterministic. An unknown tool is left to Call's own refusal.
func undeclaredArgument(tool string, args map[string]any) (ErrorPayload, bool) {
	if len(args) == 0 {
		return ErrorPayload{}, false
	}
	for _, spec := range toolSpecs() {
		if spec.Name != tool {
			continue
		}
		var schema struct {
			Properties           map[string]json.RawMessage `json:"properties"`
			AdditionalProperties *bool                      `json:"additionalProperties"`
		}
		if err := json.Unmarshal(spec.InputSchema, &schema); err != nil {
			// A schema this package wrote and cannot read is a bug here, and
			// refusing every call names it rather than accepting anything.
			return ErrorPayload{OK: false, Code: "invalid_argument",
				Error: tool + " has an unreadable input schema: " + err.Error()}, true
		}
		closed := schema.AdditionalProperties != nil && !*schema.AdditionalProperties
		names := make([]string, 0, len(args))
		for name := range args {
			names = append(names, name)
		}
		sort.Strings(names)
		for _, name := range names {
			if _, declared := schema.Properties[name]; declared || !closed {
				continue
			}
			declaredNames := make([]string, 0, len(schema.Properties))
			for p := range schema.Properties {
				declaredNames = append(declaredNames, p)
			}
			sort.Strings(declaredNames)
			msg := tool + " takes only " + strings.Join(declaredNames, ", ") + "; refusing argument " + name
			if hint, ok := argumentHints[name]; ok {
				msg += ": " + hint
			}
			return ErrorPayload{OK: false, Code: "invalid_argument", Error: msg}, true
		}
		for _, name := range names {
			property, declared := schema.Properties[name]
			if !declared {
				continue
			}
			if refusal, refused := mistypedArgument(tool, name, property, args[name]); refused {
				return refusal, true
			}
		}
		return ErrorPayload{}, false
	}
	return ErrorPayload{}, false
}

// mistypedArgument refuses a declared argument whose value is not the type, or
// not one of the values, its schema property names. Handlers read arguments
// with `args[k].(T)`, which drops a mistyped value as silently as an undeclared
// one: `force: "true"` on checkout ran an unforced checkout, `staged: "yes"`
// diffed the working tree, a numeric task_id asked about no task at all. A
// property shape this checker does not understand refuses the call rather than
// passing whatever arrived.
func mistypedArgument(tool, name string, property json.RawMessage, value any) (ErrorPayload, bool) {
	var p struct {
		Type  string `json:"type"`
		Enum  []any  `json:"enum"`
		Items *struct {
			Type string `json:"type"`
		} `json:"items"`
	}
	if err := json.Unmarshal(property, &p); err != nil {
		return ErrorPayload{OK: false, Code: "invalid_argument",
			Error: tool + " has an unreadable schema for " + name + ": " + err.Error()}, true
	}
	refuse := func(want string) (ErrorPayload, bool) {
		return ErrorPayload{OK: false, Code: "invalid_argument",
			Error: fmt.Sprintf("%s: %s must be %s, got %s", tool, name, want, jsonTypeName(value))}, true
	}
	switch p.Type {
	case "string", "boolean":
		if jsonTypeName(value) != p.Type {
			return refuse("a " + p.Type)
		}
	case "array":
		items, ok := value.([]any)
		if !ok || p.Items == nil {
			return refuse("an array")
		}
		for _, item := range items {
			if jsonTypeName(item) != p.Items.Type {
				return refuse("an array of " + p.Items.Type + "s")
			}
		}
	default:
		return ErrorPayload{OK: false, Code: "invalid_argument",
			Error: fmt.Sprintf("%s: the schema for %s declares type %q, which this host cannot check", tool, name, p.Type)}, true
	}
	if len(p.Enum) == 0 {
		return ErrorPayload{}, false
	}
	if p.Type == "array" {
		// Comparing two slices with == panics, and no schema here needs it.
		return ErrorPayload{OK: false, Code: "invalid_argument",
			Error: fmt.Sprintf("%s: the schema for %s puts an enum on an array, which this host cannot check", tool, name)}, true
	}
	allowed := make([]string, 0, len(p.Enum))
	for _, v := range p.Enum {
		if v == value {
			return ErrorPayload{}, false
		}
		allowed = append(allowed, fmt.Sprint(v))
	}
	return ErrorPayload{OK: false, Code: "invalid_argument",
		Error: fmt.Sprintf("%s: %s must be one of %s, not %q", tool, name, strings.Join(allowed, ", "), fmt.Sprint(value))}, true
}

// jsonTypeName is the JSON Schema type of a value decoded by encoding/json.
func jsonTypeName(v any) string {
	switch v.(type) {
	case nil:
		return "null"
	case string:
		return "string"
	case bool:
		return "boolean"
	case float64, json.Number:
		return "number"
	case []any:
		return "array"
	case map[string]any:
		return "object"
	default:
		return fmt.Sprintf("%T", v)
	}
}
