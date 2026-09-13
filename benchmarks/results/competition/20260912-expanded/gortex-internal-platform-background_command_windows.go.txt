//go:build windows

package platform

import (
	"os/exec"
	"syscall"

	"golang.org/x/sys/windows"
)

// ConfigureBackgroundCommand prevents a daemon-owned, non-interactive child
// process from allocating or showing a console window. CREATE_NO_WINDOW is
// incompatible with DETACHED_PROCESS and CREATE_NEW_CONSOLE, so remove those
// flags while preserving any other caller-supplied process attributes.
func ConfigureBackgroundCommand(cmd *exec.Cmd) {
	if cmd == nil {
		return
	}
	if cmd.SysProcAttr == nil {
		cmd.SysProcAttr = &syscall.SysProcAttr{}
	}
	cmd.SysProcAttr.HideWindow = true
	cmd.SysProcAttr.CreationFlags &^= windows.DETACHED_PROCESS | windows.CREATE_NEW_CONSOLE
	cmd.SysProcAttr.CreationFlags |= windows.CREATE_NO_WINDOW
}
