package devcouncil

import (
	"encoding/json"
	"fmt"
)

// ToolAnswer is one tools/call answer the way every transport renders it: the
// payload's JSON text, whether it reports an error, and the code it carries.
//
// The MCP server and `devcouncil tool call` both build it here. That is what
// makes the shell a fallback rather than a second implementation: an agent
// whose MCP call failed runs the same name with the same arguments and reads
// the same bytes an MCP client would have received.
type ToolAnswer struct {
	Text    string
	IsError bool
	// Code is the payload's "code" field, empty when it has none.
	Code string
}

// The outcomes of a tool call, as an exit status. A check that could not run
// must never read like one that ran and refused, and neither may read like a
// pass, so each has its own number.
const (
	ToolExitOK      = 0 // the tool answered ok
	ToolExitRefused = 1 // the tool ran and said no: ok:false
	ToolExitUsage   = 2 // the call itself was malformed
	ToolExitNotRun  = 3 // the tool could not run here
)

// toolUsageCodes are refusals of the call rather than of the work: an
// argument missing, undeclared, mistyped or empty. The caller fixes its
// invocation; nothing about the repository was judged.
var toolUsageCodes = map[string]bool{
	"invalid_argument": true,
	"missing_argument": true,
}

// toolNotRunCodes mean the tool never reached its question: no task store, a
// store or gate that failed, a verification that was skipped, a git probe that
// could not decide, or a call that ran out of time. Every one of them is an
// answer about this host, never about the task.
var toolNotRunCodes = map[string]bool{
	"not_initialized":       true,
	"store_error":           true,
	"policy_error":          true,
	"verify_error":          true,
	"git_repo_undetermined": true,
	"tool_error":            true,
	"timeout":               true,
}

// AnswerTool renders what Registry.Call returned. callErr is Call's own error
// (an unknown tool name), answered as code tool_error, as the MCP server always
// has.
//
// IsError keeps the server's three cases exactly: a map whose ok is false, an
// ErrorPayload or a DiffResult that is not OK. Widening it would change what
// every MCP client is told, which is not this function's decision to make.
func AnswerTool(payload any, callErr error) ToolAnswer {
	if callErr != nil {
		return ToolAnswer{
			Text:    MarshalToolPayload(map[string]any{"ok": false, "error": callErr.Error(), "code": "tool_error"}),
			IsError: true,
			Code:    "tool_error",
		}
	}
	text := MarshalToolPayload(payload)
	isError := false
	switch p := payload.(type) {
	case map[string]any:
		if okFlag, exists := p["ok"].(bool); exists && !okFlag {
			isError = true
		}
	case ErrorPayload:
		isError = !p.OK
	case DiffResult:
		isError = !p.OK
	}
	return ToolAnswer{Text: text, IsError: isError, Code: payloadCode(text)}
}

// TimeoutAnswer is the answer for a call that outlived its budget.
func TimeoutAnswer(name, budget, env string) ToolAnswer {
	return ToolAnswer{
		Text: MarshalToolPayload(map[string]any{
			"ok": false, "code": "timeout",
			"error": fmt.Sprintf("%s did not answer within %s; set %s (milliseconds) to change the budget", name, budget, env),
		}),
		IsError: true,
		Code:    "timeout",
	}
}

// ExitStatus is the answer read as a process exit status.
//
// An ok:false payload the server does not flag (a type outside AnswerTool's
// three cases) still exits non-zero here: a shell caller branching on the
// status must not read a refusal as success. Its text is unchanged.
func (a ToolAnswer) ExitStatus() int {
	refused := a.IsError
	if !refused {
		var probe struct {
			OK *bool `json:"ok"`
		}
		if json.Unmarshal([]byte(a.Text), &probe) == nil && probe.OK != nil && !*probe.OK {
			refused = true
		}
	}
	switch {
	case !refused:
		return ToolExitOK
	case toolUsageCodes[a.Code]:
		return ToolExitUsage
	case toolNotRunCodes[a.Code]:
		return ToolExitNotRun
	default:
		return ToolExitRefused
	}
}

// MarshalToolPayload is the one encoder for tool payload text. It stays on
// encoding/json v1 deliberately: v2 writes nil slices and maps as [] and {}
// where v1 writes null, so moving it is a wire-contract change for every MCP
// client and every golden envelope, made on purpose or not at all.
func MarshalToolPayload(v any) string {
	b, err := json.Marshal(v)
	if err != nil {
		return `{"ok":false,"error":"marshal failed"}`
	}
	return string(b)
}

// ServedToolSpecs is every tool this host serves, from the one literal list
// tools/list and Call read. A caller with no Registry (the shell's tool list)
// reads it here rather than spelling a list of its own.
func ServedToolSpecs() []ToolSpec { return toolSpecs() }

// ToolsListResult is the tools/list result for specs: the name, description,
// input schema and behaviour annotations of each, in order.
func ToolsListResult(specs []ToolSpec) (map[string]any, error) {
	tools := make([]map[string]any, 0, len(specs))
	for _, spec := range specs {
		var schema any
		if err := json.Unmarshal(spec.InputSchema, &schema); err != nil {
			return nil, fmt.Errorf("tool %s inputSchema: %w", spec.Name, err)
		}
		tools = append(tools, map[string]any{
			"name":        spec.Name,
			"description": spec.Description,
			"inputSchema": schema,
			"annotations": spec.Behaviour.Annotations(),
		})
	}
	return map[string]any{"tools": tools}, nil
}

func payloadCode(text string) string {
	var probe struct {
		Code string `json:"code"`
	}
	if json.Unmarshal([]byte(text), &probe) != nil {
		return ""
	}
	return probe.Code
}
