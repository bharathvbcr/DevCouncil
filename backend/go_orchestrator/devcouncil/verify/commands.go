package verify

import (
	"os/exec"
	"strings"
)

// Signatures that mean the command could not run (tooling/plan defect), so a
// non-zero exit proves nothing about the implementation.
var malformedSignatures = []string{
	"syntaxerror",
	"invalid syntax",
	"indentationerror",
	"no module named",
	"can't open file",
	"no such file or directory",
	"file or directory not found",
	"no tests ran",
	"no tests collected",
	"error: not found",
	"is not recognized as an internal or external command",
	"command not found",
	"executable file not found",
	"failed to run command",
	"importerror",
	"modulenotfounderror",
}

// CommandIsMalformed reports whether a non-zero outcome is unrunnable tooling
// rather than a real test failure.
func CommandIsMalformed(outcome CommandOutcome) bool {
	if outcome.Skipped {
		return true
	}
	text := strings.ToLower(outcome.Summary + "\n" + outcome.Stdout + "\n" + outcome.Stderr)
	for _, sig := range malformedSignatures {
		if strings.Contains(text, sig) {
			return true
		}
	}
	// Exit -1 is the synthetic "could not launch" used by the default runner.
	if outcome.ExitCode < 0 {
		return true
	}
	return false
}

// RunVerificationCommands executes planner/config verification commands.
//
// Invariant: a command that could not run yields invalid_verification_command
// or skipped_verification_command — never a silent pass.
func RunVerificationCommands(taskID string, commands []string, run func(string) CommandOutcome) []Gap {
	if len(commands) == 0 {
		return nil
	}
	if run == nil {
		var gaps []Gap
		for _, cmd := range commands {
			c := cmd
			gaps = append(gaps, Gap{
				ID:       StableGapID(taskID, "SKIP-"+cmd),
				Severity: "low",
				GapType:  "skipped_verification_command",
				TaskID:   taskID,
				Description: "Skipped verification command '" + cmd +
					"': command runner unavailable.",
				Evidence:         []string{"command runner unavailable"},
				RecommendedFix:   "Replace it with a command for this repo's stack, or remove it from the task's expected_tests.",
				Blocking:         false,
				SuggestedCommand: &c,
			})
		}
		return gaps
	}
	var gaps []Gap
	for _, cmd := range commands {
		c := cmd
		outcome := run(cmd)
		if outcome.Skipped {
			reason := outcome.Reason
			if reason == "" {
				reason = "not applicable"
			}
			gaps = append(gaps, Gap{
				ID:               StableGapID(taskID, "SKIP-"+cmd),
				Severity:         "low",
				GapType:          "skipped_verification_command",
				TaskID:           taskID,
				Description:      "Skipped verification command '" + cmd + "': " + reason + ".",
				Evidence:         []string{reason},
				RecommendedFix:   "Replace it with a command for this repo's stack, or remove it from the task's expected_tests.",
				Blocking:         false,
				SuggestedCommand: &c,
			})
			continue
		}
		if outcome.ExitCode == 0 {
			continue
		}
		summary := outcome.Summary
		if summary == "" {
			summary = strings.TrimSpace(outcome.Stderr)
		}
		if summary == "" {
			summary = strings.TrimSpace(outcome.Stdout)
		}
		if CommandIsMalformed(outcome) {
			if summary == "" {
				summary = "Failed to run command"
			}
			gaps = append(gaps, Gap{
				ID:       StableGapID(taskID, "BADCMD-"+cmd),
				Severity: "medium",
				GapType:  "invalid_verification_command",
				TaskID:   taskID,
				Description: "Verification command could not run (not a code failure): '" + cmd +
					"'. It appears malformed or its tooling is unavailable, so this command " +
					"proves nothing either way.",
				Evidence: []string{truncate(summary, 500)},
				RecommendedFix: "Regenerate the task's verification commands with 'dev repair', or edit them " +
					"to be a single runnable command (e.g. 'python -m pytest <file>').",
				Blocking:         false,
				SuggestedCommand: &c,
			})
			continue
		}
		gaps = append(gaps, Gap{
			ID:               StableGapID(taskID, "TESTFAIL-"+cmd),
			Severity:         "high",
			GapType:          "test_failed",
			TaskID:           taskID,
			Description:      "Verification command failed: '" + cmd + "'.",
			Evidence:         []string{truncate(summary, 500)},
			RecommendedFix:   "Fix the failing check, then re-run: " + cmd,
			Blocking:         true,
			SuggestedCommand: &c,
		})
	}
	return gaps
}

// DefaultRunCommand shells out through /bin/sh -c. A missing executable or
// launch failure is reported as exit -1 with a "Failed to run command: …"
// summary so CommandIsMalformed classifies it as invalid, never as a pass.
func DefaultRunCommand(root string) func(string) CommandOutcome {
	return func(command string) CommandOutcome {
		cmd := exec.Command("/bin/sh", "-c", command)
		cmd.Dir = root
		out, err := cmd.CombinedOutput()
		text := string(out)
		if err != nil {
			if ee, ok := err.(*exec.ExitError); ok {
				return CommandOutcome{
					ExitCode: ee.ExitCode(),
					Summary:  truncate(text, 2000),
					Stdout:   text,
					Stderr:   text,
				}
			}
			return CommandOutcome{
				ExitCode: -1,
				Summary:  "Failed to run command: " + err.Error(),
				Stdout:   text,
				Stderr:   err.Error(),
			}
		}
		return CommandOutcome{ExitCode: 0, Summary: truncate(text, 2000), Stdout: text}
	}
}

func truncate(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n]
}
