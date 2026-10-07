package devcouncil_test

import (
	"context"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil"
)

// A registry with no gate has nothing to judge a write with. It used to answer
// `ok: true, allowed: true`, so a host whose gate failed to build reported
// every path — `.env` included — as writable.
func TestPolicyCheckWithoutAGateIsNotAVerdict(t *testing.T) {
	reg := devcouncil.NewRegistry(t.TempDir(), nil, nil)
	for _, path := range []string{"src/a.go", ".env"} {
		out, err := reg.Call(context.Background(), "devcouncil_policy_check_write", map[string]any{"path": path})
		if err != nil {
			t.Fatalf("%s: %v", path, err)
		}
		payload, ok := out.(devcouncil.ErrorPayload)
		if !ok {
			t.Fatalf("%s: got %T %+v, want an ErrorPayload", path, out, out)
		}
		if payload.OK || payload.Code != "not_initialized" {
			t.Errorf("%s: payload = %+v, want ok=false code=not_initialized", path, payload)
		}
	}
}
