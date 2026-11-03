package store

import (
	"slices"
	"testing"
)

// signals.record is the only way a host reports a CI result. A method missing
// from WorkbenchMethods is refused with unknown_method before any process
// starts, so the watcher would never reach the store.
func TestWorkbenchMethodsAllowSignalsRecord(t *testing.T) {
	if !slices.Contains(WorkbenchMethods(), "signals.record") {
		t.Fatal("WorkbenchMethods must list signals.record")
	}
}
