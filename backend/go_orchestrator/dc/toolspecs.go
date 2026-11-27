package dc

import (
	"bytes"
	"encoding/json"
)

// SharedToolBehaviour is (readOnly, destructive, idempotent, openWorld).
type SharedToolBehaviour struct {
	ReadOnly    bool
	Destructive bool
	Idempotent  bool
	OpenWorld   bool
}

// Annotations maps behaviour into the MCP ToolAnnotations shape.
func (b SharedToolBehaviour) Annotations() map[string]any {
	return map[string]any{
		"readOnlyHint":    b.ReadOnly,
		"destructiveHint": b.Destructive,
		"idempotentHint":  b.Idempotent,
		"openWorldHint":   b.OpenWorld,
	}
}

// SharedToolSpec is one canonical shared devcouncil_* tool specification.
type SharedToolSpec struct {
	Name        string
	Description string
	InputSchema json.RawMessage
	Behaviour   SharedToolBehaviour
}

var sharedRO = SharedToolBehaviour{ReadOnly: true, Destructive: false, Idempotent: true, OpenWorld: false}

// repoPathSchemaProps declares the optional per-call repository selector accepted
// by every canonical devcouncil_* tool so a single global `devcouncil mcp` server
// can serve multiple repositories while remaining backward-compatible when omitted.
const repoPathSchemaProps = `"repo_path":{"type":"string","description":"Optional absolute path to the target repository root. Defaults to the server's configured repository root."},"root":{"type":"string","description":"Alias for repo_path: optional absolute path to the target repository root."}`

var canonicalSharedTools = []SharedToolSpec{
	{
		Name:        "devcouncil_get_diff",
		Description: "Return the working-tree (or staged) git diff, optionally scoped to a task's planned files or explicit paths.",
		InputSchema: json.RawMessage(`{"type":"object","properties":{"task_id":{"type":"string"},"paths":{"type":"array","items":{"type":"string"}},"staged":{"type":"boolean"},` + repoPathSchemaProps + `},"additionalProperties":false}`),
		Behaviour:   sharedRO,
	},
	{
		Name:        "devcouncil_checkout_task",
		Description: "Acquire a lease on a task and return its scope, lease token, and allowed next tools.",
		InputSchema: json.RawMessage(`{"type":"object","properties":{"task_id":{"type":"string"},"client_id":{"type":"string"},"agent":{"type":"string"},"force":{"type":"boolean","description":"Take the task even if another client holds its lease, revoking that lease. The reply names the revoked owner as revoked_lease_owner. Reserved for human intervention."},` + repoPathSchemaProps + `},"required":["task_id","client_id"],"additionalProperties":false}`),
		Behaviour:   SharedToolBehaviour{ReadOnly: false, Destructive: false, Idempotent: false, OpenWorld: false},
	},
	{
		Name:        "devcouncil_renew_lease",
		Description: "Extend an active lease before its TTL expires.",
		InputSchema: json.RawMessage(`{"type":"object","properties":{"task_id":{"type":"string"},"lease_token":{"type":"string"},` + repoPathSchemaProps + `},"required":["task_id","lease_token"],"additionalProperties":false}`),
		Behaviour:   SharedToolBehaviour{ReadOnly: false, Destructive: false, Idempotent: false, OpenWorld: false},
	},
	{
		Name:        "devcouncil_release_task",
		Description: "Release a held lease so another agent can check the task out.",
		InputSchema: json.RawMessage(`{"type":"object","properties":{"task_id":{"type":"string"},"lease_token":{"type":"string"},` + repoPathSchemaProps + `},"required":["task_id","lease_token"],"additionalProperties":false}`),
		Behaviour:   SharedToolBehaviour{ReadOnly: false, Destructive: false, Idempotent: true, OpenWorld: false},
	},
	{
		Name:        "devcouncil_next_task",
		Description: "Find the next ready, unheld task. Does not acquire a lease.",
		InputSchema: json.RawMessage(`{"type":"object","properties":{"client_id":{"type":"string"},` + repoPathSchemaProps + `},"additionalProperties":false}`),
		Behaviour:   sharedRO,
	},
	{
		Name:        "devcouncil_verify_task",
		Description: "Verify a leased task. Requires a valid lease_token. Verifies the task's diff against its scope and acceptance criteria and returns the verdict: status, blocking gaps, and typed next actions to repair them. The result shape is pinned by committed golden envelopes captured from the retired Python implementation.",
		InputSchema: json.RawMessage(`{"type":"object","properties":{"task_id":{"type":"string"},"lease_token":{"type":"string"},` + repoPathSchemaProps + `},"required":["task_id","lease_token"],"additionalProperties":false}`),
		Behaviour:   SharedToolBehaviour{ReadOnly: false, Destructive: false, Idempotent: true, OpenWorld: true},
	},
	{
		Name:        "devcouncil_get_gaps",
		Description: "Return verification gaps for a task from the latest persisted verification run.",
		InputSchema: json.RawMessage(`{"type":"object","properties":{"task_id":{"type":"string"},` + repoPathSchemaProps + `},"required":["task_id"],"additionalProperties":false}`),
		Behaviour:   sharedRO,
	},
	{
		Name:        "devcouncil_policy_check_write",
		Description: "Ask whether a write to a path would be allowed under the current gate posture. Pass task_id to judge it against that task's planned scope; without one the answer stops at task.absent after the secret, restricted and outside-root rules. operation is create, modify (the default) or delete.",
		InputSchema: json.RawMessage(`{"type":"object","properties":{"path":{"type":"string"},"task_id":{"type":"string"},"operation":{"type":"string","enum":["create","modify","delete"]},` + repoPathSchemaProps + `},"required":["path"],"additionalProperties":false}`),
		Behaviour:   sharedRO,
	},
}

func cloneSharedToolSpec(spec SharedToolSpec) SharedToolSpec {
	spec.InputSchema = bytes.Clone(spec.InputSchema)
	return spec
}

// SharedToolSpecs returns a copy of the 8 canonical shared devcouncil_* tool
// specifications in declaration order.
func SharedToolSpecs() []SharedToolSpec {
	out := make([]SharedToolSpec, len(canonicalSharedTools))
	for i, spec := range canonicalSharedTools {
		out[i] = cloneSharedToolSpec(spec)
	}
	return out
}

// SharedToolNames returns the 8 canonical shared devcouncil_* tool names in
// declaration order.
func SharedToolNames() []string {
	names := make([]string, len(canonicalSharedTools))
	for i, spec := range canonicalSharedTools {
		names[i] = spec.Name
	}
	return names
}

// SharedToolSpecByName looks up one of the 8 canonical shared devcouncil_*
// tool specifications by exact name.
func SharedToolSpecByName(name string) (SharedToolSpec, bool) {
	for _, spec := range canonicalSharedTools {
		if spec.Name == name {
			return cloneSharedToolSpec(spec), true
		}
	}
	return SharedToolSpec{}, false
}
