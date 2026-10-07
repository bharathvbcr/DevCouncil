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

// TASK-P7-2: only local execution exists, so any other sandbox is refused as a
// usage error before a store is opened or a command runs. It used to be
// accepted and written onto the report as the sandbox the run used.
func TestVerifyRefusesASandboxItCannotProvide(t *testing.T) {
	for _, value := range []string{"docker", "nix", "bogus"} {
		stderr, restore := swapStderr(t)
		code := dispatch([]string{"verify", "TASK-1", "--sandbox", value})
		restore()

		if code != 2 {
			t.Fatalf("--sandbox %s: exit %d, want 2; stderr=%s", value, code, stderr.String())
		}
		if !strings.Contains(stderr.String(), value) {
			t.Errorf("--sandbox %s: the refusal must name the value: %q", value, stderr.String())
		}
	}
}

// The two usage texts used to disagree: the top-level usage advertised
// local|docker|nix while `verify -h` said local.
func TestVerifyUsageTextsAgreeOnTheSandbox(t *testing.T) {
	stdout, restoreOut := swapStdout(t)
	stderr, restoreErr := swapStderr(t)
	dispatch([]string{"verify", "--help"})
	restoreErr()
	restoreOut()

	help := stdout.String() + stderr.String()
	if !strings.Contains(help, "[--sandbox local]") {
		t.Errorf("verify --help does not say [--sandbox local]: %q", help)
	}

	topErr, restoreTop := swapStderr(t)
	usage()
	restoreTop()
	var top string
	for _, line := range strings.Split(topErr.String(), "\n") {
		if strings.Contains(line, "devcouncil verify ") {
			top = line
		}
	}
	if !strings.Contains(top, "[--sandbox local]") {
		t.Errorf("top-level usage line for verify does not say [--sandbox local]: %q", top)
	}
	for _, text := range []string{help, top} {
		if strings.Contains(text, "docker") || strings.Contains(text, "nix]") {
			t.Errorf("a usage text still advertises an isolation that does not exist")
		}
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
