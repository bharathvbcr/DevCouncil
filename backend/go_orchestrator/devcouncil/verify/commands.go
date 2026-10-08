package verify

import (
	"context"
	"errors"
	"fmt"
	"os/exec"
	"strings"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
)

// CommandTimeout bounds one verification command. A verification command is
// usually a whole test suite, so the bound is generous: it exists so that a
// hung suite ends the run with a failure instead of holding `devcouncil verify`
// and the MCP verify tool open forever.
const CommandTimeout = 30 * time.Minute

// commandOutputLimit bounds what one command's output may occupy in memory.
// The stored summary was already cut to 2000 bytes; this bounds the copy that
// summary is cut from.
const commandOutputLimit = 4 << 20

// commandWaitDelay bounds the wait for the output pipes to close after the
// command's process group is killed.
const commandWaitDelay = 2 * time.Second

// CommandIsMalformed uses the runner's structured outcome. Child output is
// untrusted text: a failing test may print any tooling error without changing
// the fact that its process ran and failed. A command stopped by its deadline
// ran; it is not malformed.
func CommandIsMalformed(outcome CommandOutcome) bool {
	return !outcome.TimedOut && (outcome.Skipped || outcome.ExitCode < 0)
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
				Blocking:         true,
				SuggestedCommand: &c,
			})
		}
		return gaps
	}
	var gaps []Gap
	for _, cmd := range commands {
		c := cmd
		outcome := run(cmd)
		// First, before the exit code is read: a stopped command reports -1,
		// which is otherwise the "could not run" shape.
		if outcome.TimedOut {
			reason := outcome.Reason
			if reason == "" {
				reason = "stopped before it finished"
			}
			gaps = append(gaps, Gap{
				ID:               StableGapID(taskID, "TIMEOUT-"+cmd),
				Severity:         "high",
				GapType:          "test_failed",
				TaskID:           taskID,
				Description:      "Verification command did not finish: '" + cmd + "' (" + reason + ").",
				Evidence:         []string{truncate(outcome.Summary, 500)},
				RecommendedFix:   "Find what hangs or runs long, fix it, then re-run: " + cmd,
				Blocking:         true,
				SuggestedCommand: &c,
			})
			continue
		}
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
				Blocking:         true,
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
				Blocking:         true,
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

// DefaultRunCommand shells out through /bin/sh -c in root, each command bounded
// by CommandTimeout and by ctx. A missing executable or launch failure is
// reported as exit -1 with a "Failed to run command: …" summary so
// CommandIsMalformed classifies it as invalid, never as a pass.
func DefaultRunCommand(ctx context.Context, root string) func(string) CommandOutcome {
	return runCommand(ctx, root, CommandTimeout, commandOutputLimit)
}

// runCommand is DefaultRunCommand with its bounds as parameters.
//
// The command runs in its own process group, so the deadline reaches whatever
// it started: a test runner's workers would otherwise outlive it and hold the
// output pipe open. A stopped command is TimedOut with exit -1, and Reason says
// whether its deadline passed or the caller cancelled the run.
func runCommand(ctx context.Context, root string, timeout time.Duration, limit int) func(string) CommandOutcome {
	return func(command string) CommandOutcome {
		cctx, cancel := context.WithTimeout(ctx, timeout)
		defer cancel()

		// #nosec G204 -- the command is the task's own verification command,
		// which this runner exists to execute; see docs/security.md.
		cmd := exec.CommandContext(cctx, "/bin/sh", "-c", command)
		proc.ConfigureGroup(cmd)
		cmd.Dir = root
		out := &proc.CappedBuffer{Limit: limit}
		cmd.Stdout = out
		cmd.Stderr = out
		cmd.WaitDelay = commandWaitDelay

		err, abandoned := proc.RunBounded(cctx, cmd.Run)
		if abandoned || cctx.Err() != nil {
			// When RunBounded gave up first, the copy goroutine may still be
			// writing the buffer, so it is not read on this path.
			reason := stopReason(ctx, cctx, timeout)
			return CommandOutcome{
				ExitCode: -1,
				TimedOut: true,
				Reason:   reason,
				Summary:  "stopped: " + reason,
			}
		}

		text := out.String()
		summary := truncate(text, 2000)
		if out.Overflowed() {
			summary = fmt.Sprintf("[output truncated at %d bytes]\n%s", limit, summary)
		}
		if err != nil {
			var ee *exec.ExitError
			if errors.As(err, &ee) {
				return CommandOutcome{
					ExitCode: ee.ExitCode(),
					Summary:  summary,
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
		return CommandOutcome{ExitCode: 0, Summary: summary, Stdout: text}
	}
}

// stopReason says why a command was stopped: its own deadline, or the caller
// cancelling (or timing out) the whole verification.
func stopReason(parent, cmdCtx context.Context, timeout time.Duration) string {
	switch {
	case errors.Is(parent.Err(), context.Canceled):
		return "verification was cancelled"
	case parent.Err() != nil:
		return "the verification's deadline was reached"
	case errors.Is(cmdCtx.Err(), context.DeadlineExceeded):
		return fmt.Sprintf("deadline of %s reached", timeout)
	default:
		return "stopped before it finished"
	}
}

func truncate(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n]
}
