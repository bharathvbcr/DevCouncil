//go:build windows

package verify

import (
	"os/exec"
	"syscall"
)

// setShellCommandLine writes the verification command's whole Windows command
// line, because the one Go would build from cmd.Args escapes with backslashes
// and cmd.exe does not read that convention. Whatever ConfigureGroup put in
// SysProcAttr is kept.
func setShellCommandLine(cmd *exec.Cmd, shell, command string) {
	if cmd.SysProcAttr == nil {
		cmd.SysProcAttr = &syscall.SysProcAttr{}
	}
	cmd.SysProcAttr.CmdLine = windowsShellCommandLine(shell, command)
}
