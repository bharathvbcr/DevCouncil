package main

import (
	"context"
	"fmt"
	"os"
	"os/signal"
	"strings"
	"syscall"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/console"
)

func runCLI(args []string) int {
	rest, policy, err := presentationArgs(args)
	if err != nil {
		return console.Run(policy, context.Background(), func() int { console.Errorln(err); return 2 })
	}
	if policy.Protocol {
		return dispatch(rest)
	}
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	return console.Run(policy, ctx, func() int { return dispatch(rest) })
}

// hookIsManagement separates `dev hook status|disable` from the retired
// lifecycle events that share the word.
//
// An event call must stay inert: no render session, no goroutine, nothing on
// either stream. The two management subcommands are ordinary commands a person
// or a script runs, and `--json` owes them one JSON value on stdout like any
// other command. Only the session can keep that promise on the paths where the
// handler has no receipt to write — `integrate.Uninstall` returns none for an
// unsupported `--client`, and with the protocol bypass that left stdout empty
// while a caller was parsing it.
func hookIsManagement(args []string) bool {
	return len(args) > 1 && (args[1] == "status" || args[1] == "disable")
}

func presentationArgs(args []string) ([]string, console.Policy, error) {
	policy := console.Policy{Title: "Command", Activity: "Checking the request", Progress: "auto"}
	// Delegated commands own their presentation and complete argv grammar.
	if len(args) > 0 && (args[0] == "map" || args[0] == "graph" || args[0] == "ast" || (args[0] == "hook" && !hookIsManagement(args)) || args[0] == "mcp" || args[0] == "mcp-server") {
		policy.Protocol = true
		return args, policy, nil
	}
	values := map[string]bool{"--project-root": true, "--prefix": true, "--sandbox": true, "--mode": true, "--skill": true, "--target": true, "--hook": true, "--client": true}
	var rest []string
	for i := 0; i < len(args); i++ {
		arg := args[i]
		if arg == "--" {
			rest = append(rest, args[i:]...)
			break
		}
		if arg == "--progress" || strings.HasPrefix(arg, "--progress=") {
			value := strings.TrimPrefix(arg, "--progress=")
			if arg == "--progress" {
				i++
				if i == len(args) {
					return nil, policy, fmt.Errorf("--progress needs auto, always, or never")
				}
				value = args[i]
			}
			if value != "auto" && value != "always" && value != "never" {
				return nil, policy, fmt.Errorf("invalid --progress value %q (use auto, always, or never)", value)
			}
			policy.Progress = value
			continue
		}
		rest = append(rest, arg)
		if values[arg] && i+1 < len(args) {
			i++
			rest = append(rest, args[i])
			continue
		}
		if arg == "--json" {
			policy.JSON = true
		}
		if arg == "--help" || arg == "-h" || arg == "--version" || arg == "-V" || arg == "-v" {
			policy.Protocol = true
		}
	}
	if len(rest) == 0 {
		policy.Protocol = true
		return rest, policy, nil
	}
	switch rest[0] {
	case "integrate", "integrations":
		policy.Title, policy.Activity = "Host integration", "Connecting your tools"
		policy.JSON = true
	case "skills":
		policy.Title, policy.Activity = "Skills", "Packing your toolkit"
		for _, arg := range rest[1:] {
			if arg == "--dry-run" || arg == "--check" {
				policy.JSON = true
			}
		}
	case "verify":
		policy.Title, policy.Activity = "Verification", "Checking the work"
	case "install":
		policy.Title, policy.Activity = "Installation", "Preparing your tools"
	case "uninstall":
		policy.Title, policy.Activity = "Uninstall", "Checking the installed tools"
	case "disable", "enable":
		policy.Title, policy.Activity = "Components", "Updating component preferences"
	case "gate":
		policy.Title, policy.Activity = "Gate settings", "Reading the gate settings"
	case "map", "graph", "ast":
		// Progress flags before the alias belong to the child too.
		rest = append(rest, "--progress", policy.Progress)
		policy.Protocol = true
	case "hook":
		// Reached only for the management subcommands; the event path took the
		// protocol return above.
		policy.Title, policy.Activity = "Hooks", "Checking host registrations"
	case "mcp", "mcp-server", "version", "--version", "-V", "-v", "help", "--help", "-h":
		policy.Protocol = true
	default:
		policy.Title, policy.Activity = "Command", "Checking the request"
	}
	return rest, policy, nil
}
