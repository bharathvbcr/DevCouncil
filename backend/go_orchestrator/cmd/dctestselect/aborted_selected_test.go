package main

import (
	"strings"
	"testing"
)

// The selected phase is read with the same Qualify rule as the full suite. A
// test binary that prints `test result: FAILED` at column zero and then aborts
// exits non-zero with no failing test named, which its own exit status
// contradicts: the fast phase is not examined.
func TestASelectedBinaryThatPrintsAResultLineThenAbortsIsNotExamined(t *testing.T) {
	aborted := strings.Join([]string{
		"     Running unittests src/lib.rs (target/debug/deps/cd-0123456789abcdef)",
		"",
		"running 2 tests",
		"test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s",
		"error: test failed, to rerun pass `-p cd --lib`",
		"",
		"Caused by:",
		"  process didn't exit successfully: `/x/cd-0123` (signal: 6, SIGABRT: process abort signal)",
		"",
	}, "\n")
	dir := phaseDir(t, selectedCa, cargoCaPassed, goPass, finished(0))
	selectedStatus(t, dir, selectedCa, ExitInfo{Code: 101, Detail: "exit status 101"})
	writeFile(t, dir, "selected-rust.log", aborted)
	code, stdout, r := auditIn(t, dir)
	if code != 2 || !strings.Contains(stdout, "the selected phase was not examined") {
		t.Fatalf("exit=%d: an aborted binary was read as an examined fast phase\n%s", code, stdout)
	}
	if r.SelectedPhaseFailedOnly != nil && len(r.SelectedPhaseFailedOnly) != 0 {
		t.Errorf("selected_phase_failed_only = %v", r.SelectedPhaseFailedOnly)
	}
}
