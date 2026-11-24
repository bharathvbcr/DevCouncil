//go:build !unix && !windows

package proc

import "os/exec"

// cancelOnWindows has nothing to add where os/exec's own cancel is enough.
func cancelOnWindows(cmd *exec.Cmd) func() error { return nil }
