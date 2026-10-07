package devcouncil_test

import (
	"context"
	"encoding/json"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil"
)

// The class, not the case: every served tool refuses an argument its own
// schema does not declare. A dropped argument is a configuration the caller
// believes it made, on any tool — `force` misspelled on checkout, `staged` sent
// as `stage` to get_diff.
func TestEveryToolRefusesArgumentsItsSchemaDoesNotDeclare(t *testing.T) {
	reg := devcouncil.NewRegistry(t.TempDir(), nil, nil)
	for _, spec := range reg.Specs() {
		out, err := reg.Call(context.Background(), spec.Name, map[string]any{"not_in_the_schema": true})
		if err != nil {
			t.Fatalf("%s: %v", spec.Name, err)
		}
		payload, ok := out.(devcouncil.ErrorPayload)
		if !ok || payload.Code != "invalid_argument" || !strings.Contains(payload.Error, "not_in_the_schema") {
			t.Errorf("%s: got %T %+v, want invalid_argument naming the argument", spec.Name, out, out)
		}
	}
}

// The same class one step on: a declared argument of the wrong type is dropped
// as silently as an undeclared one, because handlers read `args[k].(T)`.
// `force: "true"` ran an unforced checkout and `staged: "yes"` diffed the
// working tree. Every declared property, on every tool, refuses a value of
// another JSON type before any lease, store or gate is consulted.
func TestEveryToolRefusesArgumentsOfTheWrongType(t *testing.T) {
	reg := devcouncil.NewRegistry(t.TempDir(), nil, nil)
	wrong := map[string][]any{
		"string":  {float64(7), true, nil, []any{"x"}, map[string]any{}},
		"boolean": {"true", float64(1), nil},
		"array":   {"src/a.go", []any{float64(7)}, []any{"ok", nil}},
	}
	checked := 0
	for _, spec := range reg.Specs() {
		var schema struct {
			Properties map[string]struct {
				Type string `json:"type"`
			} `json:"properties"`
		}
		if err := json.Unmarshal(spec.InputSchema, &schema); err != nil {
			t.Fatalf("%s: %v", spec.Name, err)
		}
		for name, prop := range schema.Properties {
			values, known := wrong[prop.Type]
			if !known {
				t.Fatalf("%s.%s: no wrong-type values for schema type %q", spec.Name, name, prop.Type)
			}
			for _, v := range values {
				out, err := reg.Call(context.Background(), spec.Name, map[string]any{name: v})
				if err != nil {
					t.Fatalf("%s %s=%#v: %v", spec.Name, name, v, err)
				}
				payload, ok := out.(devcouncil.ErrorPayload)
				if !ok || payload.Code != "invalid_argument" || !strings.Contains(payload.Error, name) {
					t.Errorf("%s %s=%#v: got %T %+v, want invalid_argument naming %s", spec.Name, name, v, out, out, name)
				}
				checked++
			}
		}
	}
	if checked == 0 {
		t.Fatal("no property was checked; the schemas could not be read")
	}

	// The drops named above, written out so they do not depend on reading the
	// schema back, plus an enum: operation takes only what its schema lists.
	for _, tc := range []struct {
		tool string
		args map[string]any
		name string
	}{
		{"devcouncil_checkout_task", map[string]any{"task_id": "T", "client_id": "c", "force": "true"}, "force"},
		{"devcouncil_get_diff", map[string]any{"staged": "yes"}, "staged"},
		{"devcouncil_get_diff", map[string]any{"paths": []any{"a.go", float64(7)}}, "paths"},
		{"devcouncil_policy_check_write", map[string]any{"path": "a.go", "operation": "write"}, "operation"},
		{"devcouncil_policy_check_write", map[string]any{"path": "a.go", "operation": "MODIFY"}, "operation"},
	} {
		out, err := reg.Call(context.Background(), tc.tool, tc.args)
		if err != nil {
			t.Fatalf("%s %v: %v", tc.tool, tc.args, err)
		}
		payload, ok := out.(devcouncil.ErrorPayload)
		if !ok || payload.Code != "invalid_argument" || !strings.Contains(payload.Error, tc.name) {
			t.Errorf("%s %v: got %T %+v, want invalid_argument naming %s", tc.tool, tc.args, out, out, tc.name)
		}
	}
}

// TASK-P7-2, MCP half. devcouncil_verify_task's schema has no sandbox field,
// and nothing enforces a tool schema at runtime, so a caller that sent
// `sandbox: "docker"` used to have it dropped without a word and get back a
// report — which it would read as the isolated run it asked for. An argument
// the tool does not take is refused before the lease or the store is touched.
func TestVerifyTaskRefusesArgumentsOutsideItsSchema(t *testing.T) {
	reg := devcouncil.NewRegistry(t.TempDir(), nil, nil)
	for _, extra := range []map[string]any{
		{"sandbox": "docker"},
		{"sandbox": "nix"},
		{"coverage": "cover.out"},
	} {
		args := map[string]any{"task_id": "TASK-1", "lease_token": "tok"}
		var name string
		for k, v := range extra {
			args[k] = v
			name = k
		}
		out, err := reg.Call(context.Background(), "devcouncil_verify_task", args)
		if err != nil {
			t.Fatalf("%v: %v", extra, err)
		}
		payload, ok := out.(devcouncil.ErrorPayload)
		if !ok {
			t.Fatalf("%v: got %T %+v, want an ErrorPayload", extra, out, out)
		}
		if payload.OK || payload.Code != "invalid_argument" {
			t.Errorf("%v: payload = %+v, want ok=false code=invalid_argument", extra, payload)
		}
		if !strings.Contains(payload.Error, name) {
			t.Errorf("%v: the refusal must name the argument: %q", extra, payload.Error)
		}
	}
}
