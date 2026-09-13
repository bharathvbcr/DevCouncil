//go:build !windows

// Package platform isolates the handful of OS-specific primitives Gortex
// needs at runtime — the set of signals that trigger a graceful
// shutdown, process liveness and termination, and the SysProcAttr that
// detaches a spawned daemon. Keeping these behind one package is what
// lets the rest of the tree compile unchanged on every supported OS.
package platform

import (
	"os"
	"syscall"
)

// ShutdownSignals returns the signals a long-running process should trap
// to begin a graceful shutdown. On Unix that's SIGINT (Ctrl-C) and
// SIGTERM (the default `kill` / supervisor stop signal).
func ShutdownSignals() []os.Signal {
	return []os.Signal{syscall.SIGINT, syscall.SIGTERM}
}

// ProcessAlive reports whether a process with the given PID currently
// exists. Signalling 0 is the canonical Unix liveness probe: it runs
// every permission check but delivers nothing, so a nil error means the
// process is there (and reachable).
func ProcessAlive(pid int) bool {
	if pid <= 0 {
		return false
	}
	return syscall.Kill(pid, 0) == nil
}

// TerminateProcess asks the process to exit gracefully (SIGTERM).
func TerminateProcess(pid int) error {
	if pid <= 0 {
		return nil
	}
	return syscall.Kill(pid, syscall.SIGTERM)
}

// KillProcess forcibly terminates the process (SIGKILL).
func KillProcess(pid int) error {
	if pid <= 0 {
		return nil
	}
	return syscall.Kill(pid, syscall.SIGKILL)
}

// DetachSysProcAttr returns the SysProcAttr that detaches a spawned
// child from the parent's controlling terminal — Setsid puts the child
// in its own session, so Ctrl-C in the parent shell isn't forwarded to
// the daemon.
func DetachSysProcAttr() *syscall.SysProcAttr {
	return &syscall.SysProcAttr{Setsid: true}
}
