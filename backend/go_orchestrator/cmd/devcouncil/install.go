package main

import (
	"fmt"
	"os"
	"strings"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/components"
)

func runInstall(args []string) int {
	list, jsonOut, dry, prefix, names, help, err := parseInstallArgs(args)
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		return 2
	}
	if help {
		fmt.Print(components.HelpText)
		return 0
	}
	if list {
		if jsonOut {
			b, err := components.ListJSON()
			if err != nil {
				fmt.Fprintf(os.Stderr, "install: %v\n", err)
				return 1
			}
			fmt.Println(string(b))
			return 0
		}
		fmt.Print(components.HelpText)
		return 0
	}
	cs, err := components.Resolve(names)
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		return 2
	}
	if err := components.Install(cs, components.Options{Prefix: prefix, DryRun: dry}); err != nil {
		fmt.Fprintf(os.Stderr, "install: %v\n", err)
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
				fmt.Fprintln(os.Stderr, "--prefix needs a directory")
				return 2
			}
			prefix = args[i]
		default:
			if strings.HasPrefix(args[i], "--prefix=") {
				prefix = strings.TrimPrefix(args[i], "--prefix=")
				if prefix == "" {
					fmt.Fprintln(os.Stderr, "--prefix needs a directory")
					return 2
				}
				continue
			}
			if strings.HasPrefix(args[i], "-") {
				fmt.Fprintf(os.Stderr, "unknown flag: %s\n", args[i])
				return 2
			}
			names = append(names, args[i])
		}
	}
	if help {
		fmt.Fprintln(os.Stderr, "usage: devcouncil uninstall [names…] [--all] [--yes] [--prefix DIR] [--dry-run]")
		return 0
	}
	if all && len(names) > 0 {
		fmt.Fprintln(os.Stderr, "uninstall: do not mix --all with component names")
		return 2
	}
	if !all && len(names) == 0 {
		fmt.Fprintln(os.Stderr, "uninstall requires a component name or --all (see --help)")
		return 2
	}
	if all {
		names = nil
	}
	cs, err := components.Resolve(names)
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		return 2
	}
	root := prefix
	if root == "" {
		root = components.DefaultPrefix()
	}
	if !yes && !dry {
		fmt.Fprintf(os.Stderr, "This will remove %d component(s) from %s. Re-run with --yes.\n", len(cs), root)
		return 2
	}
	if err := components.Uninstall(cs, components.Options{Prefix: prefix, DryRun: dry, Yes: yes}); err != nil {
		fmt.Fprintf(os.Stderr, "uninstall: %v\n", err)
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
	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "-h", "--help":
			help = true
		case "--dry-run":
			dry = true
		case "--prefix":
			i++
			if i >= len(args) {
				fmt.Fprintln(os.Stderr, "--prefix needs a directory")
				return 2
			}
			prefix = args[i]
		default:
			if strings.HasPrefix(args[i], "--prefix=") {
				prefix = strings.TrimPrefix(args[i], "--prefix=")
				if prefix == "" {
					fmt.Fprintln(os.Stderr, "--prefix needs a directory")
					return 2
				}
				continue
			}
			if strings.HasPrefix(args[i], "-") {
				fmt.Fprintf(os.Stderr, "unknown flag: %s\n", args[i])
				return 2
			}
			if name != "" {
				fmt.Fprintf(os.Stderr, "unexpected argument: %s\n", args[i])
				return 2
			}
			name = args[i]
		}
	}
	if help {
		fmt.Fprintf(os.Stderr, "usage: devcouncil %s NAME [--prefix DIR]\n", verb)
		fmt.Fprintln(os.Stderr, "NAME is a component id, or 'hooks' to uninstall write-gate hooks.")
		return 0
	}
	if name == "" {
		fmt.Fprintf(os.Stderr, "%s requires NAME (see --help)\n", verb)
		return 2
	}
	if strings.EqualFold(name, "hooks") {
		if !disable {
			fmt.Fprintln(os.Stderr, "enable hooks: re-run `devcouncil integrate HOST --write-gate`")
			return 2
		}
		return runIntegrate([]string{"uninstall", "--target", "hooks"})
	}
	opts := components.Options{Prefix: prefix, DryRun: dry}
	var err error
	if disable {
		err = components.Disable(name, opts)
	} else {
		err = components.Enable(name, opts)
	}
	if err != nil {
		fmt.Fprintf(os.Stderr, "%s: %v\n", verb, err)
		return 1
	}
	return 0
}
