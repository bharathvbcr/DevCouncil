package store

import (
	"context"
	"errors"
	"slices"
	"testing"
)

// signals.record is deliberately NOT part of the Go profile API. An earlier
// revision listed it (and pinned that it was listed) so a Go host could report
// CI results; nothing on the Go side ever did. Its only writer is GitPulse,
// which calls the Rust dc-store in-process, and a Go host such as Manvi serve
// copies WorkbenchMethods() into the ops it serves, so listing it would hand
// every serve client a write that records CI results into task attention.
// Access like that is opt-in, so these tests reverse the earlier pin on
// purpose: the method stays out, and the Rust dispatcher and the dcstore CLI
// keep it.
func TestWorkbenchMethodsExcludeSignalsRecord(t *testing.T) {
	if slices.Contains(WorkbenchMethods(), "signals.record") {
		t.Fatal("WorkbenchMethods must not list signals.record: its only writer is GitPulse in-process, and a Go host would expose it to its clients")
	}
}

// The allowlist check runs before any process is spawned, so a client whose
// binary does not exist still answers: an unknown_method refusal proves the
// call never reached the spawn path (which would fail with a different error).
func TestWorkbenchRefusesSignalsRecordBeforeSpawn(t *testing.T) {
	c := New("/nonexistent/dcstore-must-never-run", "/nonexistent/store.db")
	_, err := c.Workbench(context.Background(), "signals.record", []byte(`{}`))
	var refusal *WorkbenchError
	if !errors.As(err, &refusal) {
		t.Fatalf("signals.record must be refused by the allowlist, got %v", err)
	}
	if refusal.Code != "unknown_method" {
		t.Fatalf("refusal code = %q, want unknown_method", refusal.Code)
	}
}
