//go:build !windows

package platform

import "os/exec"

// ConfigureBackgroundCommand is a no-op on Unix. Daemon-owned subprocesses
// inherit the daemon's detached session without allocating a separate window.
func ConfigureBackgroundCommand(_ *exec.Cmd) {}
