//go:build windows

package proc

import (
	"errors"
	"os"
	"os/exec"
	"syscall"
)

const (
	// processQueryLimitedInformation is the least access that answers
	// GetExitCodeProcess.
	processQueryLimitedInformation = 0x1000
	// stillActive is the exit code a process reports until it has exited.
	stillActive = 259
)

// cancelOnWindows is cmd.Cancel for a child with no process group here: it
// kills the direct child, and reports a child that had already exited as
// os.ErrProcessDone, which os/exec treats as nothing to cancel.
//
// os.Process.Kill on Windows calls TerminateProcess, which fails with
// ERROR_ACCESS_DENIED for a process that has exited but has not been reaped
// yet, and os/exec then fails Wait with "canceling Cmd: TerminateProcess:
// Access is denied" for a command that finished by itself, as the context
// ended. That is not a failure to cancel, and reporting it as one made every
// child that exited just as its context was cancelled look like an error. The
// error is mapped only after asking the system whether the process is gone: an
// access-denied for a live process is still an error.
func cancelOnWindows(cmd *exec.Cmd) func() error {
	return func() error {
		if cmd.Process == nil {
			return nil
		}
		err := cmd.Process.Kill()
		if err != nil && errors.Is(err, syscall.ERROR_ACCESS_DENIED) && exited(cmd.Process.Pid) {
			return os.ErrProcessDone
		}
		return err
	}
}

// exited reports whether the process with this pid has exited. A pid it cannot
// open, or whose state it cannot read, is not reported as exited.
func exited(pid int) bool {
	h, err := syscall.OpenProcess(processQueryLimitedInformation, false, uint32(pid))
	if err != nil {
		return false
	}
	defer syscall.CloseHandle(h) //nolint:errcheck // nothing to do about a failed close
	var code uint32
	if err := syscall.GetExitCodeProcess(h, &code); err != nil {
		return false
	}
	return code != stillActive
}
