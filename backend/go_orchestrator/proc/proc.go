// Package proc owns the subprocess wait bound shared by every MANVI process
// boundary.
package proc

import (
	"context"
	"os"
	"os/exec"
	"time"
)

// RunBounded runs one subprocess invocation under the caller's deadline and
// reports whether the deadline won.
//
// exec.CommandContext arms its killer only after the child exists, and
// Cmd.WaitDelay also starts after Start returns. A Start blocked in the kernel
// is therefore outside both bounds. Running the complete call here makes the
// caller's context bound cover Start as well as Wait.
//
// When the deadline wins, the goroutine is abandoned. The caller must not read
// state the goroutine may still be writing, such as an output buffer.
func RunBounded(ctx context.Context, run func() error) (err error, timedOut bool) {
	return runBounded(ctx, run, 0)
}

// RunBoundedWithCleanup gives CommandContext up to 100 ms to kill and reap
// children after cancellation. CLI callers use it before exiting the process;
// a stuck Start or escaped descendant still cannot hold that exit indefinitely.
func RunBoundedWithCleanup(ctx context.Context, run func() error) (err error, timedOut bool) {
	return runBounded(ctx, run, 100*time.Millisecond)
}

func runBounded(ctx context.Context, run func() error, cleanup time.Duration) (err error, timedOut bool) {
	done := make(chan error, 1)
	go func() { done <- run() }()
	select {
	case err = <-done:
		return err, false
	case <-ctx.Done():
		if cleanup > 0 {
			select {
			case <-done:
			case <-time.After(cleanup):
			}
		}
		return nil, true
	}
}

// EnforceSubprocessInvariants applies mandatory subprocess isolation invariants:
// 1. Stdin redirected to /dev/null if nil to prevent hangs waiting for interactive input.
// 2. Process group isolation with signal escalation cancellation (SIGTERM -> grace -> SIGKILL).
// 3. Bounded WaitDelay (500ms) for stdio drain threads.
// 4. Suppression of interactive terminal prompts, added to the inherited
// environment (a nil cmd.Env is filled from os.Environ first).
//
// Like ConfigureGroup, it sets cmd.Cancel, so cmd must come from
// exec.CommandContext.
func EnforceSubprocessInvariants(cmd *exec.Cmd) {
	if cmd.Stdin == nil {
		if devNull, err := os.Open(os.DevNull); err == nil {
			cmd.Stdin = devNull
		}
	}
	ConfigureGroup(cmd)
	if cmd.WaitDelay == 0 {
		cmd.WaitDelay = 500 * time.Millisecond
	}
	cmd.Cancel = func() error {
		if cmd.Process == nil {
			return nil
		}
		KillGroupWithEscalation(cmd.Process.Pid, 500*time.Millisecond)
		return nil
	}
	// A nil Env inherits the parent's environment; appending to nil would
	// replace it with these two variables alone.
	if cmd.Env == nil {
		cmd.Env = os.Environ()
	}
	cmd.Env = append(cmd.Env, "GIT_TERMINAL_PROMPT=0", "GCM_INTERACTIVE=never")
}
