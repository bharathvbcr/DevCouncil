package verify

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"
)

// These pin dc-verify-local-runner-unbounded. Before it, DefaultRunCommand ran
// `/bin/sh -c` through exec.Command and CombinedOutput: no deadline, no
// cancellation, no bound on what it buffered, and a hung test hung
// `devcouncil verify` and MCP devcouncil_verify_task with it.

// runWithin runs one command and fails the test, rather than hanging it, when
// the runner does not return within limit. A runner with no bound would
// otherwise turn this suite into the hang it is checking for.
func runWithin(t *testing.T, limit time.Duration, run func(string) CommandOutcome, command string) CommandOutcome {
	t.Helper()
	done := make(chan CommandOutcome, 1)
	go func() { done <- run(command) }()
	select {
	case out := <-done:
		return out
	case <-time.After(limit):
		t.Fatalf("the runner did not return within %s for %q", limit, command)
		return CommandOutcome{}
	}
}

// waitGone polls until pid no longer exists, within a bounded wait.
func waitGone(t *testing.T, pid int) {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		if err := syscall.Kill(pid, 0); errors.Is(err, syscall.ESRCH) {
			return
		}
		time.Sleep(20 * time.Millisecond)
	}
	t.Fatalf("pid %d is still running after the runner returned", pid)
}

func TestARunawayCommandIsKilledWithEverythingItStarted(t *testing.T) {
	root := t.TempDir()
	pidFile := filepath.Join(root, "child.pid")
	ctx, cancel := context.WithTimeout(context.Background(), 300*time.Millisecond)
	defer cancel()

	start := time.Now()
	out := runWithin(t, 15*time.Second, DefaultRunCommand(ctx, root),
		"sleep 60 & echo $! > "+pidFile+"; wait")
	if elapsed := time.Since(start); elapsed > 10*time.Second {
		t.Fatalf("returned after %s; the deadline was 300ms", elapsed)
	}
	if !out.TimedOut || out.ExitCode != -1 {
		t.Fatalf("outcome = %+v, want TimedOut with exit -1", out)
	}
	if !strings.Contains(out.Reason, "deadline") {
		t.Errorf("reason %q should say the deadline was reached", out.Reason)
	}

	raw, err := os.ReadFile(pidFile)
	if err != nil {
		t.Fatalf("the command never started its child: %v", err)
	}
	pid, err := strconv.Atoi(strings.TrimSpace(string(raw)))
	if err != nil {
		t.Fatalf("pid file %q: %v", raw, err)
	}
	waitGone(t, pid)
}

func TestACancelledVerificationStopsTheCommandItIsRunning(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	time.AfterFunc(200*time.Millisecond, cancel)

	out := runWithin(t, 15*time.Second, DefaultRunCommand(ctx, t.TempDir()), "sleep 60")
	if !out.TimedOut || out.ExitCode != -1 {
		t.Fatalf("outcome = %+v, want a stopped command with exit -1", out)
	}
	if !strings.Contains(out.Reason, "cancelled") {
		t.Errorf("reason %q should say the run was cancelled, not that it timed out", out.Reason)
	}
}

func TestCommandOutputIsCappedWithoutChangingTheVerdict(t *testing.T) {
	const limit = 1024
	run := runCommand(context.Background(), t.TempDir(), time.Minute, limit)

	out := runWithin(t, 30*time.Second, run, "head -c 1000000 /dev/zero | tr '\\0' x; exit 0")
	if out.ExitCode != 0 || out.TimedOut {
		t.Fatalf("outcome = %+v; a command that exited 0 still passed", out)
	}
	if len(out.Stdout) > limit {
		t.Fatalf("kept %d bytes of output, the cap is %d", len(out.Stdout), limit)
	}
	if !strings.Contains(out.Summary, "truncated") {
		t.Errorf("summary %q should say the output was truncated", out.Summary)
	}

	failing := runWithin(t, 30*time.Second, run, "head -c 1000000 /dev/zero | tr '\\0' y; exit 3")
	if failing.ExitCode != 3 {
		t.Fatalf("outcome = %+v; the exit code must survive the cap", failing)
	}
}

func TestAWellBehavedCommandStillReportsItsOutputAndExitCode(t *testing.T) {
	run := DefaultRunCommand(context.Background(), t.TempDir())
	ok := runWithin(t, 30*time.Second, run, "echo hello")
	if ok.ExitCode != 0 || ok.TimedOut || strings.TrimSpace(ok.Stdout) != "hello" {
		t.Fatalf("outcome = %+v", ok)
	}
	bad := runWithin(t, 30*time.Second, run, "echo nope >&2; exit 7")
	if bad.ExitCode != 7 || !strings.Contains(bad.Summary, "nope") {
		t.Fatalf("outcome = %+v", bad)
	}
}

// A timeout is the command failing, not the command being malformed. Before
// this, a stopped command came back as exit -1 and was filed as
// invalid_verification_command, "proves nothing either way".
func TestATimedOutCommandIsABlockingTestFailure(t *testing.T) {
	gaps := RunVerificationCommands("TASK-T", []string{"go test ./..."}, func(string) CommandOutcome {
		return CommandOutcome{ExitCode: -1, TimedOut: true, Reason: "deadline of 30m0s reached",
			Summary: "stopped: deadline of 30m0s reached"}
	})
	if len(gaps) != 1 {
		t.Fatalf("gaps = %+v, want one", gaps)
	}
	g := gaps[0]
	if g.GapType != "test_failed" || !g.Blocking {
		t.Fatalf("gap = %+v, want a blocking test_failed", g)
	}
	if !strings.Contains(g.Description, "did not finish") {
		t.Errorf("description %q should say the command did not finish", g.Description)
	}
}
