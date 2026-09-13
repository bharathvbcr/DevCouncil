package main

import (
	"fmt"
	"os"
)

// runHook is the compatibility boundary for retired lifecycle commands. Event
// calls must never read stdin, open project state, spawn workers or emit host
// decisions. Even malformed legacy flags and future events are inert. Human
// management subcommands have explicit validation and failure receipts.
func runHook(args []string) int {
	if len(args) == 0 {
		return 0
	}
	switch args[0] {
	case "status":
		return runIntegrateUninstall(append([]string{"--check"}, args[1:]...))
	case "disable":
		return runIntegrateUninstall(args[1:])
	case "enable":
		fmt.Fprintln(os.Stderr, "DevCouncil lifecycle hooks are retired; use DevMap's own integration for index maintenance and DevCouncil MCP tools for verification.")
		return 1
	case "--help", "-h", "help":
		fmt.Fprint(os.Stdout, hookHelp)
	}
	return 0
}

const hookHelp = `DevCouncil lifecycle hooks are retired.
Legacy event invocations exit 0 silently without reading stdin or project state.

  dev hook status [--project-root DIR] [--client HOST] [--json]
  dev hook disable [--project-root DIR] [--client HOST] [--dry-run] [--json]
  dev disable hooks [--project-root DIR] [--client HOST] [--dry-run]

Status is read-only: exit 0 means clean, exit 1 means registrations remain or
inspection failed (see the receipt). Cleanup preserves other tools' hooks,
creates backups, and only scans the selected root. Client defaults to all:
claude, cursor, codex, gemini, grok, opencode.
Use --project-root with your home directory to inspect user settings explicitly.
Reload the host session after cleanup. MCP policy and host permissions are unchanged.
`
