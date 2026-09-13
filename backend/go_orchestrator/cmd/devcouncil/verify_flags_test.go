package main

import (
	"strings"
	"testing"
)

// `--coverage` is what makes the diff∩coverage gate reachable at all from a
// shipped command: MCP's tool schema has no field for a path, so without this
// flag the gate would exist in the library and be callable by nothing.

func TestVerifyCoverageFlagNeedsAValue(t *testing.T) {
	stderr, restore := swapStderr(t)
	code := dispatch([]string{"verify", "TASK-1", "--coverage"})
	restore()

	if code != 2 {
		t.Fatalf("exit %d, want 2 for a flag with no value; stderr=%s", code, stderr.String())
	}
	// A flag that silently consumed nothing would leave coverage unmeasured
	// while the operator believed they had asked for it — the report would say
	// "coverage profile not supplied" for a run where one was.
	if !strings.Contains(stderr.String(), "--coverage") {
		t.Errorf("the refusal must name the flag: %q", stderr.String())
	}
}

func TestVerifyHelpNamesTheCoverageFlag(t *testing.T) {
	stdout, restoreOut := swapStdout(t)
	stderr, restoreErr := swapStderr(t)
	code := dispatch([]string{"verify", "--help"})
	restoreErr()
	restoreOut()

	if code != 0 {
		t.Fatalf("exit %d, stderr=%s", code, stderr.String())
	}
	got := stdout.String() + stderr.String()
	if !strings.Contains(got, "--coverage") {
		t.Errorf("verify --help does not mention --coverage: %q", got)
	}
}

// TestVerifyStillRefusesUnknownFlags is the positive control for the parser
// change: adding a case to that switch must not turn it into one that accepts
// anything. An unknown flag silently ignored is an operator who thinks they
// configured a run they did not.
func TestVerifyRefusesUnknownFlags(t *testing.T) {
	stderr, restore := swapStderr(t)
	code := dispatch([]string{"verify", "TASK-1", "--coverag"})
	restore()

	if code != 2 {
		t.Fatalf("exit %d, want 2 for an unknown flag; stderr=%s", code, stderr.String())
	}
	if !strings.Contains(stderr.String(), "unknown flag") {
		t.Errorf("stderr=%q", stderr.String())
	}
}
