//go:build windows

package verify

import (
	"context"
	"strings"
	"testing"
	"time"
)

// On a Windows host the runner reaches cmd.exe and a trivial command passes.
// Before verifyShell this was `/bin/sh -c`, which does not exist there, so every
// expected command came back as a launch failure.
func TestATrivialCommandPassesOnWindows(t *testing.T) {
	run := runCommand(context.Background(), t.TempDir(), time.Minute, 1<<16)
	out := run(`echo verify-shell-ok`)
	if out.ExitCode != 0 || out.TimedOut {
		t.Fatalf("exit %d timedOut=%v summary=%q", out.ExitCode, out.TimedOut, out.Summary)
	}
	if !strings.Contains(out.Stdout, "verify-shell-ok") {
		t.Fatalf("stdout %q does not carry the echoed text", out.Stdout)
	}
}

// The command line is passed through as typed, quotes included, rather than
// re-escaped with the backslash convention cmd.exe does not read.
func TestQuotesReachCmdAsTyped(t *testing.T) {
	run := runCommand(context.Background(), t.TempDir(), time.Minute, 1<<16)
	out := run(`if "a b"=="a b" (echo same) else (echo differ)`)
	if out.ExitCode != 0 || !strings.Contains(out.Stdout, "same") {
		t.Fatalf("exit %d stdout=%q", out.ExitCode, out.Stdout)
	}
}
