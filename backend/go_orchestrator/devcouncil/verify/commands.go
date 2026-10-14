package verify

import (
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"runtime"
	"strings"
	"time"
	"unicode/utf8"

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
	gaps, _ := runVerificationCommands(taskID, commands, run)
	return gaps
}

// CommandRecord is the evidence one verification command leaves behind: what
// ran, how it ended and how long it took, with a bounded tail of its output.
// It is what the stored run's commands_json carries, so a run that executed
// `go vet ./...` and `false` is distinguishable from one that executed nothing.
type CommandRecord struct {
	Command    string `json:"command"`
	ExitCode   int    `json:"exit_code"`
	DurationMS int64  `json:"duration_ms"`
	TimedOut   bool   `json:"timed_out,omitempty"`
	// Skipped marks a command that did not run (no runner, or refused).
	Skipped bool   `json:"skipped,omitempty"`
	Reason  string `json:"reason,omitempty"`
	Tail    string `json:"output_tail,omitempty"`
}

// commandTailLimit bounds the output kept per command record.
const commandTailLimit = 2000

func commandRecord(cmd string, outcome CommandOutcome, took time.Duration) CommandRecord {
	tail := outcome.Summary
	if tail == "" {
		tail = strings.TrimSpace(outcome.Stderr)
	}
	if tail == "" {
		tail = strings.TrimSpace(outcome.Stdout)
	}
	return CommandRecord{
		Command:    cmd,
		ExitCode:   outcome.ExitCode,
		DurationMS: took.Milliseconds(),
		TimedOut:   outcome.TimedOut,
		Skipped:    outcome.Skipped,
		Reason:     outcome.Reason,
		Tail:       truncateTail(tail, commandTailLimit),
	}
}

// truncateTail keeps the end of s: the last lines of a failing run are the
// ones that name the failure.
func truncateTail(s string, limit int) string {
	if len(s) <= limit {
		return s
	}
	cut := len(s) - limit
	for cut < len(s) && !utf8.RuneStart(s[cut]) {
		cut++
	}
	return s[cut:]
}

func runVerificationCommands(taskID string, commands []string, run func(string) CommandOutcome) ([]Gap, []CommandRecord) {
	if len(commands) == 0 {
		return nil, nil
	}
	if run == nil {
		var gaps []Gap
		records := make([]CommandRecord, 0, len(commands))
		for _, cmd := range commands {
			c := cmd
			records = append(records, CommandRecord{Command: cmd, ExitCode: -1, Skipped: true, Reason: "command runner unavailable"})
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
		return gaps, records
	}
	var gaps []Gap
	records := make([]CommandRecord, 0, len(commands))
	for _, cmd := range commands {
		c := cmd
		began := time.Now()
		outcome := run(cmd)
		records = append(records, commandRecord(cmd, outcome, time.Since(began)))
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
			Evidence:         []string{"exit code " + itoa(outcome.ExitCode) + ": " + truncate(summary, 500)},
			RecommendedFix:   "Fix the failing check, then re-run: " + cmd,
			Blocking:         true,
			SuggestedCommand: &c,
		})
	}
	return gaps, records
}

// DefaultRunCommand shells out through verifyShell (/bin/sh -c, or cmd.exe on
// Windows) in root, each command bounded by CommandTimeout and by ctx. A missing executable or launch failure is
// reported as exit -1 with a "Failed to run command: …" summary so
// CommandIsMalformed classifies it as invalid, never as a pass.
func DefaultRunCommand(ctx context.Context, root string) func(string) CommandOutcome {
	return RunCommandWithEnv(ctx, root, LoadEnvPolicy(root))
}

// RunCommandWithEnv is DefaultRunCommand with the environment policy already
// resolved, for a caller that also records the policy (VerifyTask does).
func RunCommandWithEnv(ctx context.Context, root string, policy EnvPolicy) func(string) CommandOutcome {
	return runCommandWithPolicy(ctx, root, CommandTimeout, commandOutputLimit, policy)
}

// runCommand is DefaultRunCommand with its bounds as parameters.
//
// The command runs in its own process group, so the deadline reaches whatever
// it started: a test runner's workers would otherwise outlive it and hold the
// output pipe open. A stopped command is TimedOut with exit -1, and Reason says
// whether its deadline passed or the caller cancelled the run.
func runCommand(ctx context.Context, root string, timeout time.Duration, limit int) func(string) CommandOutcome {
	return runCommandWithPolicy(ctx, root, timeout, limit, EnvPolicy{})
}

func runCommandWithPolicy(ctx context.Context, root string, timeout time.Duration, limit int, policy EnvPolicy) func(string) CommandOutcome {
	return func(command string) CommandOutcome {
		cctx, cancel := context.WithTimeout(ctx, timeout)
		defer cancel()

		shell, args, err := verifyShell(runtime.GOOS, os.Getenv, command)
		if err != nil {
			// Exit -1 with "Failed to run command" is the launch-failure shape
			// CommandIsMalformed reads as invalid, never as a pass.
			return CommandOutcome{
				ExitCode: -1,
				Summary:  "Failed to run command: " + err.Error(),
				Stderr:   err.Error(),
			}
		}
		// #nosec G204 -- the command is the task's own verification command,
		// which this runner exists to execute; see docs/security.md.
		cmd := exec.CommandContext(cctx, shell, args...)
		proc.ConfigureGroup(cmd)
		setShellCommandLine(cmd, shell, command)
		cmd.Dir = root
		// Allowlist, not inherit: a nil Env would hand the command every
		// secret in the host process (docs/verify-sandbox-security-impact.md).
		cmd.Env = policy.Environ(os.Environ())
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

// verifyShell names the shell a verification command runs under on goos, and
// the arguments that follow it.
//
// Everywhere but Windows that is `/bin/sh -c`. On Windows there is no
// /bin/sh, so it is cmd.exe from ComSpec (or SystemRoot's System32 when
// ComSpec is unset), run as `/d /s /c`: /d skips the AutoRun registry hook so
// nothing the host's registry adds runs with the command, and /s /c is the
// quoting rule windowsShellCommandLine writes the line for.
//
// It is a pure function of goos and getenv so the Windows branch is exercised
// by `go test` on every host, not only on a Windows job.
//
// Refusals, each failing closed with its reason:
//   - no ComSpec and no SystemRoot: there is no shell to name;
//   - a shell path that is not absolute (bare, `.\`, `C:x`, `\x`): exec would
//     resolve it against cmd.Dir, the repository under verification, and run a
//     cmd.exe the repository supplied;
//   - a quote in the shell path, which would end the quoted program name early;
//   - a line break in the command: cmd.exe runs only the first line, so the
//     rest would be reported as verified without having run.
func verifyShell(goos string, getenv func(string) string, command string) (string, []string, error) {
	if goos != "windows" {
		return "/bin/sh", []string{"-c", command}, nil
	}
	shell := getenv("ComSpec")
	source := "ComSpec"
	if shell == "" {
		root := getenv("SystemRoot")
		if root == "" {
			return "", nil, errors.New("neither ComSpec nor SystemRoot is set, so there is no cmd.exe to run the verification command with")
		}
		shell = strings.TrimRight(root, `\/`) + `\System32\cmd.exe`
		source = "SystemRoot"
	}
	if strings.Contains(shell, `"`) {
		return "", nil, fmt.Errorf("the shell named by %s (%q) contains a quote, so it cannot be written as one program name", source, shell)
	}
	if !windowsAbsolute(shell) {
		return "", nil, fmt.Errorf("the shell named by %s (%q) is not an absolute path, and a relative one would be resolved inside the repository under verification", source, shell)
	}
	if strings.ContainsAny(command, "\r\n") {
		return "", nil, errors.New("the command contains a line break, and cmd.exe would run only its first line")
	}
	return shell, []string{"/d", "/s", "/c", command}, nil
}

// windowsAbsolute reports whether p is fully qualified under Windows rules: a
// drive letter followed by a separator, or a UNC / device prefix. `C:x` (drive
// relative) and `\x` (current-drive rooted) are not. Written out rather than
// via filepath.IsAbs because that answers for the host, and verifyShell is
// tested for Windows on every host.
func windowsAbsolute(p string) bool {
	if len(p) >= 2 && isWindowsSeparator(p[0]) && isWindowsSeparator(p[1]) {
		return true
	}
	if len(p) < 3 || p[1] != ':' || !isWindowsSeparator(p[2]) {
		return false
	}
	c := p[0] | 0x20
	return c >= 'a' && c <= 'z'
}

func isWindowsSeparator(b byte) bool { return b == '\\' || b == '/' }

// windowsShellCommandLine writes the whole Windows command line for a
// verifyShell result. Go builds a Windows command line by escaping each
// argument with backslashes, a convention cmd.exe does not parse, so the line
// is written whole instead: with /s, cmd strips the first and last quote of
// what follows /c and runs everything between them exactly as typed.
func windowsShellCommandLine(shell, command string) string {
	return `"` + shell + `" /d /s /c "` + command + `"`
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
