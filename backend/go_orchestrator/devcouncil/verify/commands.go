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
			if outcome.SandboxUnavailable {
				gaps = append(gaps, Gap{
					ID:               StableGapID(taskID, "ORACLE-SANDBOX-UNAVAILABLE", cmd),
					Severity:         "critical",
					GapType:          "rigor_check_unavailable",
					TaskID:           taskID,
					Description:      "Read-only verification sandbox unavailable (ORACLE-SANDBOX-UNAVAILABLE); refusing to execute '" + cmd + "' unsandboxed: " + reason + ".",
					Evidence:         []string{"ORACLE-SANDBOX-UNAVAILABLE", reason},
					RecommendedFix:   "Install and enable the OS read-only verification sandbox (`bwrap` with user/mount namespaces on Linux, or `/usr/bin/sandbox-exec` on macOS) before re-running verification.",
					Blocking:         true,
					SuggestedCommand: &c,
				})
				continue
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
			if reason, unattested := InspectTestRunnerOutcome(cmd, outcome); unattested {
				gaps = append(gaps, Gap{
					ID:       StableGapID(taskID, "UNATTESTED-"+cmd),
					Severity: "high",
					GapType:  "unattested_test_run",
					TaskID:   taskID,
					Description: "Verification test command exited 0 without attesting executed tests: '" +
						cmd + "' (" + reason + ").",
					Evidence: []string{reason},
					RecommendedFix: "Ensure the test runner executes at least one real test and prints its " +
						"completion summary without early process exit (os._exit(0) / os.Exit(0)), then re-run: " + cmd,
					Blocking:         true,
					SuggestedCommand: &c,
				})
			}
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
		return CommandOutcome{ExitCode: 0, Summary: summary, Stdout: text, OutputCaptured: true}
	}
}

// InspectTestRunnerOutcome validates that a recognized test-runner command
// exiting 0 actually attested to executing >= 1 test and reaching its summary
// footer. Bare exit code 0 is vulnerable to early-exit and zero-test reward
// hacks (`os._exit(0)` in `conftest.py`, `std::process::exit(0)` in `#[test]`,
// `TestMain { os.Exit(0) }` under `go test -json`/`-v`, or zero-test filters).
//
// Synthetic unit-test doubles returning `CommandOutcome{ExitCode: 0}` with
// `OutputCaptured == false` and empty streams are left untouched so non-runner
// orchestration tests do not have to forge runner transcripts.
func InspectTestRunnerOutcome(command string, outcome CommandOutcome) (reason string, unattested bool) {
	if outcome.ExitCode != 0 {
		return "", false
	}
	text := outcome.Stdout
	if text == "" {
		text = outcome.Stderr
	}
	if text == "" {
		text = outcome.Summary
	}
	if !outcome.OutputCaptured && text == "" {
		return "", false
	}
	stages := classifyTestRunnerStages(command)
	if len(stages) == 0 {
		return "", false
	}
	trimmed := strings.TrimSpace(text)
	for _, st := range stages {
		kind, tokens := st.kind, st.tokens
		switch kind {
		case "pytest":
			if trimmed == "" {
				return "pytest exited 0 with empty output (possible os._exit(0) before session summary)", true
			}
			if hasFlagToken(tokens, "-qq") {
				continue
			}
			if !hasPytestSummaryFooter(text) {
				return "pytest exited 0 without reporting a `<N> passed ... in <dur>s` session summary", true
			}
		case "unittest":
			if trimmed == "" {
				return "unittest exited 0 with empty output", true
			}
			ran := countAfterPrefix(text, "Ran ")
			if ran <= 0 {
				return "unittest exited 0 with 0 tests executed", true
			}
			if !hasUnittestOKFooter(text) {
				return "unittest exited 0 without printing its `OK` completion footer", true
			}
		case "cargo_test":
			if trimmed == "" {
				return "cargo test exited 0 with empty output", true
			}
			running, okLines, passed := parseCargoTestSummary(text)
			if okLines == 0 {
				return "cargo test exited 0 without any `test result: ok.` summary (possible process::exit(0))", true
			}
			if running > okLines {
				return fmt.Sprintf("cargo test started %d test binaries but only %d emitted `test result: ok.` (early process::exit(0))", running, okLines), true
			}
			if passed <= 0 {
				return "cargo test exited 0 with 0 passed tests across all test binaries", true
			}
		case "go_test":
			if trimmed == "" {
				return "go test exited 0 with empty output", true
			}
			if hasFlagToken(tokens, "-json") {
				if !goTestJSONHasPassingTest(text) {
					return "go test -json exited 0 with zero individual `Test` pass events (possible TestMain os.Exit(0) or empty suite)", true
				}
				continue
			}
			if hasFlagToken(tokens, "-v") {
				if !strings.Contains(text, "--- PASS:") {
					return "go test -v exited 0 with zero `--- PASS:` test lines (possible TestMain os.Exit(0) or empty suite)", true
				}
				continue
			}
			if !goTestPlainHasAttestedPackage(text) {
				return "go test exited 0 with zero attested packages (`[no test files]` or `[no tests to run]`)", true
			}
		case "swift_test":
			if trimmed == "" {
				return "swift test exited 0 with empty output", true
			}
			if countAfterPrefix(text, "Executed ") <= 0 && countAfterPrefix(text, "Test run with ") <= 0 {
				return "swift test exited 0 without reporting any executed tests", true
			}
		}
	}
	return "", false
}

type runnerStage struct {
	kind   string
	tokens []string
}

func classifyTestRunnerStages(command string) []runnerStage {
	var out []runnerStage
	for _, rawStage := range splitUnquotedShellStages(command) {
		stage := trimOuterSubshellParens(rawStage)
		if stage == "" || hasUnquotedRedirectOrPipe(stage) {
			continue
		}
		if kind, tokens := classifySingleRunnerStage(stage); kind != "" {
			out = append(out, runnerStage{kind: kind, tokens: tokens})
		}
	}
	return out
}

func splitUnquotedShellStages(command string) []string {
	var stages []string
	inSingle, inDouble, escaped := false, false, false
	start := 0
	for i := 0; i < len(command); i++ {
		c := command[i]
		if escaped {
			escaped = false
			continue
		}
		if !inSingle && c == '\\' {
			escaped = true
			continue
		}
		if !inDouble && c == '\'' {
			inSingle = !inSingle
			continue
		}
		if !inSingle && c == '"' {
			inDouble = !inDouble
			continue
		}
		if inSingle || inDouble {
			continue
		}
		if c == ';' || c == '\n' {
			stages = append(stages, strings.TrimSpace(command[start:i]))
			start = i + 1
			continue
		}
		if (c == '&' || c == '|') && i+1 < len(command) && command[i+1] == c {
			stages = append(stages, strings.TrimSpace(command[start:i]))
			i++
			start = i + 1
		}
	}
	if start <= len(command) {
		stages = append(stages, strings.TrimSpace(command[start:]))
	}
	return stages
}

func trimOuterSubshellParens(s string) string {
	s = strings.TrimSpace(s)
	for strings.HasPrefix(s, "(") {
		s = strings.TrimSpace(strings.TrimPrefix(s, "("))
	}
	for strings.HasSuffix(s, ")") {
		s = strings.TrimSpace(strings.TrimSuffix(s, ")"))
	}
	return s
}

func hasUnquotedRedirectOrPipe(stage string) bool {
	inSingle, inDouble, escaped := false, false, false
	for i := 0; i < len(stage); i++ {
		c := stage[i]
		if escaped {
			escaped = false
			continue
		}
		if !inSingle && c == '\\' {
			escaped = true
			continue
		}
		if !inDouble && c == '\'' {
			inSingle = !inSingle
			continue
		}
		if !inSingle && c == '"' {
			inDouble = !inDouble
			continue
		}
		if !inSingle && !inDouble && (c == '|' || c == '>' || c == '&') {
			return true
		}
	}
	return false
}

func classifySingleRunnerStage(command string) (string, []string) {
	fields := strings.Fields(command)
	for len(fields) > 0 && isEnvAssignment(fields[0]) {
		fields = fields[1:]
	}
	if len(fields) > 0 && (fields[0] == "env" || fields[0] == "time") {
		fields = fields[1:]
		for len(fields) > 0 && (isEnvAssignment(fields[0]) || strings.HasPrefix(fields[0], "-")) {
			fields = fields[1:]
		}
	}
	if len(fields) >= 2 && (fields[0] == "uv" || fields[0] == "poetry") && fields[1] == "run" {
		fields = fields[2:]
	}
	if len(fields) == 0 {
		return "", nil
	}
	for _, tok := range fields {
		switch tok {
		case "--help", "-h", "--version", "-V", "--collect-only", "--no-run", "--list", "--list-tests":
			return "", nil
		}
	}
	prog := baseProg(fields[0])
	switch prog {
	case "pytest", "py.test":
		return "pytest", fields[1:]
	case "python", "python3":
		for i := 1; i+1 < len(fields); i++ {
			if fields[i] == "-m" {
				switch fields[i+1] {
				case "pytest":
					return "pytest", fields[i+2:]
				case "unittest":
					return "unittest", fields[i+2:]
				}
				break
			}
		}
	case "cargo":
		if idx := firstSubcommand(fields[1:]); idx >= 0 && fields[1+idx] == "test" {
			return "cargo_test", fields[2+idx:]
		}
	case "go":
		rest := skipGoGlobalFlags(fields[1:])
		if len(rest) > 0 && rest[0] == "test" {
			for _, f := range rest[1:] {
				if f == "-c" || f == "-i" || strings.HasPrefix(f, "-list") {
					return "", nil
				}
			}
			return "go_test", rest[1:]
		}
	case "swift":
		if len(fields) >= 2 && fields[1] == "test" && (len(fields) < 3 || fields[2] != "list") {
			return "swift_test", fields[2:]
		}
	}
	return "", nil
}

func isEnvAssignment(tok string) bool {
	eq := strings.IndexByte(tok, '=')
	if eq <= 0 {
		return false
	}
	for i := 0; i < eq; i++ {
		c := tok[i]
		if !((c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || c == '_' || (i > 0 && c >= '0' && c <= '9')) {
			return false
		}
	}
	return true
}

func baseProg(p string) string {
	if i := strings.LastIndexAny(p, `/\`); i >= 0 {
		return p[i+1:]
	}
	return p
}

func firstSubcommand(args []string) int {
	for i := 0; i < len(args); i++ {
		a := args[i]
		if a == "--manifest-path" || a == "--target" || a == "--profile" || a == "-C" {
			i++
			continue
		}
		if strings.HasPrefix(a, "-") || strings.HasPrefix(a, "+") {
			continue
		}
		return i
	}
	return -1
}

func skipGoGlobalFlags(args []string) []string {
	for len(args) > 0 {
		if args[0] == "-C" && len(args) >= 2 {
			args = args[2:]
			continue
		}
		if strings.HasPrefix(args[0], "-C=") {
			args = args[1:]
			continue
		}
		break
	}
	return args
}

func hasFlagToken(tokens []string, want string) bool {
	for _, tok := range tokens {
		if tok == want || strings.HasPrefix(tok, want+"=") {
			return true
		}
	}
	return false
}

func positiveCountBeforeWord(text, word string) int {
	total := 0
	fields := strings.FieldsFunc(text, func(r rune) bool {
		return r == ' ' || r == '\t' || r == '\n' || r == '\r' || r == ',' || r == ';' || r == '(' || r == ')' || r == '='
	})
	for i := 1; i < len(fields); i++ {
		if fields[i] == word {
			if n, ok := parseNonNegInt(fields[i-1]); ok && n > 0 {
				total += n
			}
		}
	}
	return total
}

func countAfterPrefix(text, prefix string) int {
	total := 0
	rest := text
	for {
		idx := strings.Index(rest, prefix)
		if idx < 0 {
			break
		}
		rest = rest[idx+len(prefix):]
		tok := strings.Fields(rest)
		if len(tok) > 0 {
			if n, ok := parseNonNegInt(tok[0]); ok {
				total += n
			}
		}
	}
	return total
}

func parseNonNegInt(s string) (int, bool) {
	if s == "" {
		return 0, false
	}
	n := 0
	for i := 0; i < len(s); i++ {
		if s[i] < '0' || s[i] > '9' {
			return 0, false
		}
		n = n*10 + int(s[i]-'0')
	}
	return n, true
}

func hasPytestSummaryFooter(text string) bool {
	for _, line := range strings.Split(text, "\n") {
		t := strings.TrimSpace(line)
		if positiveCountBeforeWord(t, "passed") <= 0 {
			continue
		}
		if hasPytestDurationSuffix(t) {
			return true
		}
	}
	return false
}

func hasPytestDurationSuffix(line string) bool {
	rest := line
	for {
		idx := strings.Index(rest, " in ")
		if idx < 0 {
			return false
		}
		rest = rest[idx+len(" in "):]
		fields := strings.Fields(rest)
		if len(fields) == 0 {
			return false
		}
		tok := strings.TrimRight(fields[0], "=)")
		if isPytestSecondsToken(tok) {
			return true
		}
	}
}

func isPytestSecondsToken(tok string) bool {
	if len(tok) < 2 || tok[len(tok)-1] != 's' {
		return false
	}
	body := tok[:len(tok)-1]
	if body[0] < '0' || body[0] > '9' {
		return false
	}
	dots := 0
	for i := 0; i < len(body); i++ {
		c := body[i]
		if c == '.' {
			dots++
			if dots > 1 || i == len(body)-1 {
				return false
			}
			continue
		}
		if c < '0' || c > '9' {
			return false
		}
	}
	return true
}

func hasUnittestOKFooter(text string) bool {
	for _, line := range strings.Split(text, "\n") {
		t := strings.TrimSpace(line)
		if t == "OK" || strings.HasPrefix(t, "OK (") {
			return true
		}
	}
	return false
}

func parseCargoTestSummary(text string) (runningBinaries, okResults, passed int) {
	for _, line := range strings.Split(text, "\n") {
		t := strings.TrimSpace(line)
		if strings.HasPrefix(t, "Running ") || strings.HasPrefix(t, "Doc-tests ") {
			runningBinaries++
		}
		if strings.HasPrefix(t, "test result: ok.") {
			okResults++
			passed += positiveCountBeforeWord(t, "passed")
		}
	}
	return runningBinaries, okResults, passed
}

func goTestJSONHasPassingTest(text string) bool {
	for _, line := range strings.Split(text, "\n") {
		t := strings.TrimSpace(line)
		if !strings.HasPrefix(t, "{") {
			continue
		}
		if strings.Contains(t, `"Action":"pass"`) && strings.Contains(t, `"Test":"`) {
			return true
		}
	}
	return false
}

func goTestPlainHasAttestedPackage(text string) bool {
	for _, line := range strings.Split(text, "\n") {
		t := strings.TrimSpace(line)
		if strings.HasPrefix(t, "ok ") || strings.HasPrefix(t, "ok\t") {
			if !strings.Contains(t, "[no tests to run]") {
				return true
			}
		}
	}
	return false
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
