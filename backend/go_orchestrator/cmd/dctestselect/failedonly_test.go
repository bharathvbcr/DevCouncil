package main

import (
	"os"
	"path/filepath"
	"testing"
)

// Selected log is examined raw, so the artifact reports selected_phase_failed_only even when the note says the fast phase was NOT examined.
func TestArtifactDoesNotReportFailedOnlyWhenSelectedPhaseWasNotExamined(t *testing.T) {
	bad := "     Running unittests src/lib.rs (t/a)\n\nrunning 1 test\ntest only_fast ... FAILED\n\nfailures:\n\n---- only_fast stdout ----\n\nfailures:\n    only_fast\n\ntest result: FAILED. 0 passed; 1 failed\nerror: test failed, to rerun pass `-p ca --lib`\n"
	dir := phaseDir(t, selectedCa, plantedMiss, goPass, finished(101))
	setSelected(t, dir, selectedCa, bad, 0) // exit 0 contradicts the failure
	code, _, r := auditIn(t, dir)
	if code == 2 && r.SelectedPhaseFailedOnly != nil {
		t.Errorf("exit 2 (fast phase not examined) yet selected_phase_failed_only=%v; the field is documented null when a phase was not examined", r.SelectedPhaseFailedOnly)
	}
}

// stale/absent selected status, but a log exists.
func TestNoSelectedStatusDoesNotReportFailedOnly(t *testing.T) {
	bad := "     Running unittests src/lib.rs (t/a)\n\nrunning 1 test\ntest only_fast ... FAILED\n\nfailures:\n\n---- only_fast stdout ----\n\nfailures:\n    only_fast\n\ntest result: FAILED. 0 passed; 1 failed\nerror: test failed, to rerun pass `-p ca --lib`\n"
	dir := phaseDir(t, selectedCa, plantedMiss, goPass, finished(101))
	setSelected(t, dir, selectedCa, bad, 101)
	if err := os.Remove(filepath.Join(dir, "selected-status.json")); err != nil {
		t.Fatal(err)
	}
	code, _, r := auditIn(t, dir)
	if code == 2 && r.SelectedPhaseFailedOnly != nil {
		t.Errorf("exit 2 yet selected_phase_failed_only=%v", r.SelectedPhaseFailedOnly)
	}
}
