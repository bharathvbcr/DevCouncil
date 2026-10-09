package main

import (
	"strings"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/console"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/components"
)

func runDisable(args []string) int {
	return runDisableEnable(args, true)
}

func runEnable(args []string) int {
	return runDisableEnable(args, false)
}

func runDisableEnable(args []string, disable bool) int {
	verb := "enable"
	if disable {
		verb = "disable"
	}
	prefix, dry, name, help := "", false, "", false
	root, client := "", ""
	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "-h", "--help":
			help = true
		case "--dry-run":
			dry = true
		case "--project-root", "--client":
			flag := args[i]
			i++
			if i >= len(args) || args[i] == "" || strings.HasPrefix(args[i], "--") {
				console.Errorf("%s needs a value\n", flag)
				return 2
			}
			if flag == "--project-root" {
				root = args[i]
			} else {
				client = args[i]
			}
		case "--prefix":
			i++
			if i >= len(args) {
				console.Errorln("--prefix needs a directory")
				return 2
			}
			prefix = args[i]
		default:
			if strings.HasPrefix(args[i], "--prefix=") {
				prefix = strings.TrimPrefix(args[i], "--prefix=")
				if prefix == "" {
					console.Errorln("--prefix needs a directory")
					return 2
				}
				continue
			}
			if strings.HasPrefix(args[i], "-") {
				console.Errorf("unknown flag: %s\n", args[i])
				return 2
			}
			if name != "" {
				console.Errorf("unexpected argument: %s\n", args[i])
				return 2
			}
			name = args[i]
		}
	}
	if help {
		console.Errorf("usage: devcouncil %s NAME [--prefix DIR]\n", verb)
		console.Errorln("NAME is a component id, or hooks to remove retired host registrations (--project-root DIR, --client HOST, --dry-run).")
		return 0
	}
	if name == "" {
		console.Errorf("%s requires NAME (see --help)\n", verb)
		return 2
	}
	if strings.EqualFold(name, "hooks") {
		if !disable {
			return runHook([]string{"enable"})
		}
		if prefix != "" {
			console.Errorln("hooks use --project-root, not --prefix")
			return 2
		}
		hookArgs := []string{}
		if dry {
			hookArgs = append(hookArgs, "--dry-run")
		}
		if root != "" {
			hookArgs = append(hookArgs, "--project-root", root)
		}
		if client != "" {
			hookArgs = append(hookArgs, "--client", client)
		}
		return runIntegrateUninstall(hookArgs)
	}
	if root != "" || client != "" {
		console.Errorln("--project-root and --client apply only to hooks")
		return 2
	}
	opts := components.Options{Prefix: prefix, DryRun: dry}
	var err error
	if disable {
		err = components.Disable(name, opts)
	} else {
		err = components.Enable(name, opts)
	}
	if err != nil {
		console.Errorf("%s: %v\n", verb, err)
		return 1
	}
	return 0
}
