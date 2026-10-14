//go:build !windows

package verify

import "os/exec"

// setShellCommandLine has nothing to do off Windows: /bin/sh receives the
// command as one argv element, exactly as cmd.Args holds it.
func setShellCommandLine(cmd *exec.Cmd, shell, command string) {}
