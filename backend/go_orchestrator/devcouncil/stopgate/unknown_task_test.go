package stopgate_test

import (
	"context"
	"path/filepath"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/stopgate"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// An unknown task id reaches the stop gate (and through it the MCP
// verify_error payload) with exactly these bytes. The CLI's not_run envelope
// is derived from the error's type, so the text must not move
// (ft-74d9ffb6).
func TestUnknownTaskSkipReasonBytesAreUnchanged(t *testing.T) {
	client := store.New(testsupport.DCStore(t), filepath.Join(t.TempDir(), "state.sqlite"))
	t.Cleanup(client.Close)
	if _, err := client.ActiveLeases(context.Background()); err != nil {
		t.Fatal(err)
	}
	out := stopgate.Run(context.Background(), stopgate.Input{
		Root: t.TempDir(), Store: client, TaskID: "NO-SUCH", GateMode: "enforce",
	})
	d := out.Decision
	if !d.Skipped || d.OK || d.Allow {
		t.Fatalf("unknown task must be a skipped, non-allowing decision: %+v", d)
	}
	if want := "verify could not run: task NO-SUCH not found"; d.SkipReason != want {
		t.Fatalf("SkipReason = %q, want %q", d.SkipReason, want)
	}
	if want := "task NO-SUCH not found"; d.Reason != want {
		t.Fatalf("Reason (the MCP verify_error text) = %q, want %q", d.Reason, want)
	}
}
