package main

import (
	"path/filepath"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/console"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/gatescfg"
)

func runGate(args []string) int {
	if len(args) < 1 {
		console.Error(gateHelp)
		return 2
	}
	switch args[0] {
	case "-h", "--help":
		console.Error(gateHelp)
		return 0
	case "status":
		return runGateStatus(args[1:])
	case "set":
		return runGateSet(args[1:])
	default:
		console.Errorf("unknown gate subcommand: %s\n", args[0])
		console.Error(gateHelp)
		return 2
	}
}

const gateHelp = `devcouncil gate — show or change verification-gate settings

Usage:
  devcouncil gate status [--json] [--project-root DIR]
  devcouncil gate set --mode off|advisory|enforce [--hook off|contain] [--project-root DIR]

Modes:
  off        skip quality verification (default; hard-safety write policy stays on)
  advisory   record non-safety findings; hard-safety gaps still block
  enforce    every blocking verification gap blocks

Host lifecycle hooks are retired. The settings that named pre-tool-use
containment are gone with them: --hook / execution.hook_gate.mode (including
its "contain" mode) and integrate's --write-gate. None of them enforced
anything. Use dev hook status to inspect stale registrations.

This command is for a human operator. Agents should not flip gates.
`

func runGateStatus(args []string) int {
	jsonOut := false
	root := projectRoot()
	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "-h", "--help":
			console.Error(gateHelp)
			return 0
		case "--json":
			jsonOut = true
		case "--project-root":
			i++
			if i >= len(args) {
				console.Errorln("--project-root needs a directory")
				return 2
			}
			root = args[i]
		default:
			console.Errorf("unknown flag: %s\n", args[i])
			return 2
		}
	}
	snap := gatescfg.Load(root)
	if jsonOut {
		if err := console.JSON(snap); err != nil {
			console.Errorf("gate: %v\n", err)
			return 1
		}
		return 0
	}
	console.Printf("verification_mode: %s (%s)\n", snap.VerificationMode, snap.VerificationOrigin)
	console.Printf("config:            %s", snap.ConfigPath)
	if !snap.ConfigPresent {
		console.Print(" (missing; defaults apply)")
	}
	console.Println()
	return 0
}

func runGateSet(args []string) int {
	root := projectRoot()
	mode := ""
	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "-h", "--help":
			console.Error(gateHelp)
			return 0
		case "--mode":
			i++
			if i >= len(args) {
				console.Errorln("--mode needs off, advisory, or enforce")
				return 2
			}
			mode = args[i]
		case "--hook":
			// Retired by name rather than left to "unknown flag": `contain`
			// was the containment mode, and a caller who set it is owed the
			// reason it went away.
			console.Errorln("--hook was removed: execution.hook_gate.mode named a " +
				"pre-tool-use gate that only DevCouncil's retired lifecycle hooks " +
				"installed. `contain` contained nothing. Use gates.mode for " +
				"verification, and the devcouncil_* MCP policy tools for write scope.")
			return 2
		case "--project-root":
			i++
			if i >= len(args) {
				console.Errorln("--project-root needs a directory")
				return 2
			}
			root = args[i]
		default:
			console.Errorf("unknown flag: %s\n", args[i])
			return 2
		}
	}
	if mode == "" {
		console.Errorln("gate set requires --mode")
		return 2
	}
	path := filepath.Join(root, ".devcouncil", "config.yaml")
	if mode != "" {
		if err := gatescfg.SetVerificationMode(path, mode); err != nil {
			console.Errorf("gate set: %v\n", err)
			return 1
		}
		canonical, _ := gatescfg.ParseMode(mode)
		console.Printf("gates.mode = %s\n", canonical)
	}
	return 0
}
