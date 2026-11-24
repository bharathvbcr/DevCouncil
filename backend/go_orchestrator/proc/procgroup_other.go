//go:build !unix

package proc

import (
	"os/exec"
	"time"
)

// ConfigureGroup arms no process group where none is available. WaitDelay
// still bounds the call, so the timeout holds; only the orphaned grandchild
// survives, which is the lesser of the two failures. On Windows it replaces
// os/exec's default cancel with one that does not report a child that had
// already exited as a failure to cancel it (see cancelOnWindows).
func ConfigureGroup(cmd *exec.Cmd) {
	if cancel := cancelOnWindows(cmd); cancel != nil {
		cmd.Cancel = cancel
	}
}

// ConfigureOwnGroup and KillGroup are no-ops where process groups are not
// available. A persistent child is still ended by closing its stdin and by the
// direct-process kill its owner falls back to, and cmd.WaitDelay still caps how
// long leftover descendants may hold the stdio pipes before Wait gives up on
// them.
//
// These exist so leaf packages that drive a persistent child compile off unix
// at all: syscall.Kill and SysProcAttr.Setpgid do not exist on every GOOS, and
// referencing them directly made packages with no other platform dependency
// refuse to build anywhere else.
func ConfigureOwnGroup(cmd *exec.Cmd) {}

func KillGroup(pid int) {}

func KillGroupWithEscalation(pid int, grace time.Duration) {}
