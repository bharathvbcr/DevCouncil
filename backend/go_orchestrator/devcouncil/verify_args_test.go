package devcouncil_test

import (
	"context"
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
