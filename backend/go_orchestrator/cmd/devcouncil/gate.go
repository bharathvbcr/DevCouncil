package main

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/gatescfg"
)

func runGate(args []string) int {
	if len(args) < 1 {
		fmt.Fprint(os.Stderr, gateHelp)
		return 2
	}
	switch args[0] {
	case "-h", "--help":
		fmt.Fprint(os.Stderr, gateHelp)
		return 0
	case "status":
		return runGateStatus(args[1:])
	case "set":
		return runGateSet(args[1:])
	default:
		fmt.Fprintf(os.Stderr, "unknown gate subcommand: %s\n", args[0])
		fmt.Fprint(os.Stderr, gateHelp)
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
			fmt.Fprint(os.Stderr, gateHelp)
			return 0
		case "--json":
			jsonOut = true
		case "--project-root":
			i++
			if i >= len(args) {
				fmt.Fprintln(os.Stderr, "--project-root needs a directory")
				return 2
			}
			root = args[i]
		default:
			fmt.Fprintf(os.Stderr, "unknown flag: %s\n", args[i])
			return 2
		}
	}
	snap := gatescfg.Load(root)
	if jsonOut {
		b, err := json.MarshalIndent(snap, "", "  ")
		if err != nil {
			fmt.Fprintf(os.Stderr, "gate: %v\n", err)
			return 1
		}
		fmt.Println(string(b))
		return 0
	}
	fmt.Printf("verification_mode: %s (%s)\n", snap.VerificationMode, snap.VerificationOrigin)
	fmt.Printf("hook_gate:         %s\n", snap.HookGate)
	fmt.Printf("write_gate:        %v\n", snap.WriteGate)
	fmt.Printf("config:            %s", snap.ConfigPath)
	if !snap.ConfigPresent {
		fmt.Print(" (missing; defaults apply)")
	}
	fmt.Println()
	return 0
}

func runGateSet(args []string) int {
	root := projectRoot()
	mode := ""
	hook := ""
	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "-h", "--help":
			fmt.Fprint(os.Stderr, gateHelp)
			return 0
		case "--mode":
			i++
			if i >= len(args) {
				fmt.Fprintln(os.Stderr, "--mode needs off, advisory, or enforce")
				return 2
			}
			mode = args[i]
		case "--hook":
			i++
			if i >= len(args) {
				fmt.Fprintln(os.Stderr, "--hook needs off or contain")
				return 2
			}
			hook = args[i]
		case "--project-root":
			i++
			if i >= len(args) {
				fmt.Fprintln(os.Stderr, "--project-root needs a directory")
				return 2
			}
			root = args[i]
		default:
			fmt.Fprintf(os.Stderr, "unknown flag: %s\n", args[i])
			return 2
		}
	}
	if mode == "" && hook == "" {
		fmt.Fprintln(os.Stderr, "gate set requires --mode and/or --hook")
		return 2
	}
	path := filepath.Join(root, ".devcouncil", "config.yaml")
	if mode != "" {
		if err := gatescfg.SetVerificationMode(path, mode); err != nil {
			fmt.Fprintf(os.Stderr, "gate set: %v\n", err)
			return 1
		}
		canonical, _ := gatescfg.ParseMode(mode)
		fmt.Printf("gates.mode = %s\n", canonical)
	}
	if hook != "" {
		if err := gatescfg.SetHookGate(path, hook); err != nil {
			fmt.Fprintf(os.Stderr, "gate set: %v\n", err)
			return 1
		}
		fmt.Printf("execution.hook_gate.mode = %s\n", strings.ToLower(hook))
	}
	return 0
}
