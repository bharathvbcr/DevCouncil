package main

import (
	"fmt"
	"strings"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/console"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/components"
)

func runInstall(args []string) int {
	list, jsonOut, dry, prefix, names, help, err := parseInstallArgs(args)
	if err != nil {
		console.Errorln(err)
		return 2
	}
	if help {
		console.Print(components.HelpText)
		return 0
	}
	if list {
		if jsonOut {
			b, err := components.ListJSON()
			if err != nil {
				console.Errorf("install: %v\n", err)
				return 1
			}
			if _, err := console.Println(string(b)); err != nil {
				console.Errorln(err)
				return 1
			}
			return 0
		}
		console.Print(components.HelpText)
		return 0
	}
	cs, err := components.Resolve(names)
	if err != nil {
		console.Errorln(err)
		return 2
	}
	opts := components.Options{Prefix: prefix, DryRun: dry}
	var installErr error
	// A dry-run plan is already structured; do not print shell commands into JSON.
	if !jsonOut || !dry {
		installErr = components.Install(cs, opts)
	}
	if jsonOut {
		ids := make([]string, 0, len(cs))
		for _, c := range cs {
			ids = append(ids, c.ID)
		}
		receipt := struct {
			OK         bool     `json:"ok"`
			DryRun     bool     `json:"dry_run"`
			Components []string `json:"components"`
			Commands   []string `json:"commands,omitempty"`
			Error      string   `json:"error,omitempty"`
		}{OK: installErr == nil, DryRun: dry, Components: ids}
		if dry {
			receipt.Commands = components.PlannedCommands(cs, opts)
		}
		if installErr != nil {
			receipt.Error = installErr.Error()
		}
		if err := console.JSON(receipt); err != nil {
			console.Errorln(err)
			return 1
		}
	}
	if installErr != nil {
		console.Errorf("install: %v\n", installErr)
		return 1
	}

	return 0
}

func parseInstallArgs(args []string) (list, jsonOut, dry bool, prefix string, names []string, help bool, err error) {
	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "-h", "--help":
			help = true
		case "--list":
			list = true
		case "--json":
			jsonOut = true
		case "--dry-run":
			dry = true
		case "--prefix":
			i++
			if i >= len(args) {
				return false, false, false, "", nil, false, fmt.Errorf("--prefix needs a directory")
			}
			prefix = args[i]
		default:
			if strings.HasPrefix(args[i], "--prefix=") {
				prefix = strings.TrimPrefix(args[i], "--prefix=")
				if prefix == "" {
					return false, false, false, "", nil, false, fmt.Errorf("--prefix needs a directory")
				}
				continue
			}
			if strings.HasPrefix(args[i], "-") {
				return false, false, false, "", nil, false, fmt.Errorf("unknown flag: %s", args[i])
			}
			names = append(names, args[i])
		}
	}
	return list, jsonOut, dry, prefix, names, help, nil
}

func runUninstall(args []string) int {
	yes, dry, all, prefix, names, help := false, false, false, "", []string{}, false
	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "-h", "--help":
			help = true
		case "--yes", "-y":
			yes = true
		case "--dry-run":
			dry = true
		case "--all":
			all = true
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
			names = append(names, args[i])
		}
	}
	if help {
		console.Errorln("usage: devcouncil uninstall [names…] [--all] [--yes] [--prefix DIR] [--dry-run]")
		return 0
	}
	if all && len(names) > 0 {
		console.Errorln("uninstall: do not mix --all with component names")
		return 2
	}
	if !all && len(names) == 0 {
		console.Errorln("uninstall requires a component name or --all (see --help)")
		return 2
	}
	if all {
		names = nil
	}
	cs, err := components.Resolve(names)
	if err != nil {
		console.Errorln(err)
		return 2
	}
	root := prefix
	if root == "" {
		root = components.DefaultPrefix()
	}
	if !yes && !dry {
		console.Errorf("This will remove %d component(s) from %s. Re-run with --yes.\n", len(cs), root)
		return 2
	}
	if err := components.Uninstall(cs, components.Options{Prefix: prefix, DryRun: dry, Yes: yes}); err != nil {
		console.Errorf("uninstall: %v\n", err)
		return 1
	}
	return 0
}

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
