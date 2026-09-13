package main

import (
	"path/filepath"
	"strings"

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

Hook gate is off unless you pass --write-gate to integrate. Changing it here
does not install hooks.

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
	console.Printf("hook_gate:         %s\n", snap.HookGate)
	console.Printf("write_gate:        %v\n", snap.WriteGate)
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
	hook := ""
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
			i++
			if i >= len(args) {
				console.Errorln("--hook needs off or contain")
				return 2
			}
			hook = args[i]
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
	if mode == "" && hook == "" {
		console.Errorln("gate set requires --mode and/or --hook")
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
	if hook != "" {
		if err := gatescfg.SetHookGate(path, hook); err != nil {
			console.Errorf("gate set: %v\n", err)
			return 1
		}
		console.Printf("execution.hook_gate.mode = %s\n", strings.ToLower(hook))
	}
	return 0
}
