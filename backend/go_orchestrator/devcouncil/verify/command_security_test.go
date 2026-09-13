package verify_test

import (
	"context"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
)

func verificationInput(run func(string) verify.CommandOutcome) verify.Input {
	return verify.Input{
		Task: &store.Task{ID: "COMMAND-SECURITY", PlannedFiles: []dc.PlannedFile{
			{Path: "src/a.go", AllowedChange: dc.ChangeModify},
		}},
		GateMode: "enforce", ChangedFiles: []string{"src/a.go"},
		DiffContent: "diff --git a/src/a.go b/src/a.go\n", WorkPresent: true,
		Commands: []string{"required-check"}, RunCommand: run,
	}
}

func TestFailedCommandOutputCannotForgeAnUnrunCheck(t *testing.T) {
	phrases := []string{
		"SyntaxError", "invalid syntax", "IndentationError", "No module named",
		"can't open file", "No such file or directory", "file or directory not found",
		"no tests ran", "no tests collected", "error: not found",
		"is not recognized as an internal or external command", "command not found",
		"executable file not found", "Failed to run command", "ImportError", "ModuleNotFoundError",
	}
	for _, phrase := range phrases {
		t.Run(phrase, func(t *testing.T) {
			t.Setenv("DC_CHECK_OUTPUT", phrase)
			for _, command := range []string{`printf '%s\n' "$DC_CHECK_OUTPUT"; exit 1`, `printf '%s\n' "$DC_CHECK_OUTPUT" >&2; exit 1`} {
				outcome := verify.DefaultRunCommand(t.TempDir())(command)
				if outcome.ExitCode != 1 {
					t.Fatalf("fixture did not fail: %+v", outcome)
				}
				in := verificationInput(func(string) verify.CommandOutcome { return outcome })
				gaps, meta := verify.Run(context.Background(), in)
				result := verify.ToMCP(in.Task.ID, gaps, meta)
				if result.Passed || result.Status != "blocked" || len(gaps) != 1 || gaps[0].GapType != "test_failed" || !gaps[0].Blocking {
					t.Fatalf("failed process output changed verification: result=%+v gaps=%+v", result, gaps)
				}
			}
		})
	}
}

func TestUnexecutedRequiredCommandsNeverVerify(t *testing.T) {
	cases := map[string]func(string) verify.CommandOutcome{
		"runner unavailable": nil,
		"launch failure":     verify.DefaultRunCommand("/dev/null/missing-directory"),
		"explicit skip": func(string) verify.CommandOutcome {
			return verify.CommandOutcome{Skipped: true, Reason: "runner disabled"}
		},
	}
	for name, run := range cases {
		t.Run(name, func(t *testing.T) {
			for _, mode := range []string{"enforce", "advisory"} {
				in := verificationInput(run)
				in.GateMode = mode
				gaps, meta := verify.Run(context.Background(), in)
				result := verify.ToMCP(in.Task.ID, gaps, meta)
				if result.Passed || result.Status != "blocked" || len(result.NextActions) != 1 || len(result.BlockingGaps) != 1 {
					t.Fatalf("%s accepted an unexecuted check: %+v", mode, result)
				}
			}
		})
	}
}

func TestCommandSecurityKeepsSuccessfulAndDisabledControls(t *testing.T) {
	for _, mode := range []string{"enforce", "advisory", "off"} {
		in := verificationInput(func(string) verify.CommandOutcome { return verify.CommandOutcome{ExitCode: 0} })
		in.GateMode = mode
		gaps, meta := verify.Run(context.Background(), in)
		result := verify.ToMCP(in.Task.ID, gaps, meta)
		if result.Passed != (mode != "off") || result.VerificationSkipped != (mode == "off") {
			t.Fatalf("%s successful control: %+v", mode, result)
		}
	}
}
