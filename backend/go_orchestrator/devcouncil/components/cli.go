package components

// The install and uninstall commands. They live here, beside the catalog
// they drive and outside the host binary, so that `cmd/dcinstall` — the
// bootstrap scripts' entry point — runs the very same code without linking
// what the host links (Gusset). `devcouncil install` and `devcouncil
// uninstall` dispatch here; there is one implementation.

import (
	"fmt"
	"strings"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/console"
)

func RunInstall(args []string) int {
	list, jsonOut, dry, prefix, names, help, err := parseInstallArgs(args)
	if err != nil {
		console.Errorln(err)
		return 2
	}
	if help {
		console.Print(HelpText)
		return 0
	}
	if list {
		if jsonOut {
			b, err := ListJSON()
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
		console.Print(HelpText)
		return 0
	}
	cs, err := Resolve(names)
	if err != nil {
		console.Errorln(err)
		return 2
	}
	opts := Options{Prefix: prefix, DryRun: dry}
	var installErr error
	// A dry-run plan is already structured; do not print shell commands into JSON.
	if !jsonOut || !dry {
		installErr = Install(cs, opts)
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
			receipt.Commands = PlannedCommands(cs, opts)
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
		case "--yes", "-y":
			// Confirmation is an uninstall question; the scripts this replaced
			// accepted it everywhere.
		case "--host-only":
			names = append(names, "host")
		case "--":
			names = append(names, args[i+1:]...)
			return list, jsonOut, dry, prefix, names, help, nil
		case "--prefix":
			i++
			if i >= len(args) {
				return false, false, false, "", nil, false, fmt.Errorf("--prefix needs a directory")
			}
			prefix = args[i]
		case "--only":
			// The bootstrap scripts' spelling of a positional name.
			i++
			if i >= len(args) || args[i] == "" {
				return false, false, false, "", nil, false, fmt.Errorf("--only needs a component name")
			}
			names = append(names, args[i])
		default:
			if strings.HasPrefix(args[i], "--only=") {
				only := strings.TrimPrefix(args[i], "--only=")
				if only == "" {
					return false, false, false, "", nil, false, fmt.Errorf("--only needs a component name")
				}
				names = append(names, only)
				continue
			}
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

func RunUninstall(args []string) int {
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
	cs, err := Resolve(names)
	if err != nil {
		console.Errorln(err)
		return 2
	}
	root := prefix
	if root == "" {
		root = DefaultPrefix()
	}
	if !yes && !dry {
		console.Errorf("This will remove %d component(s) from %s. Re-run with --yes.\n", len(cs), root)
		return 2
	}
	if err := Uninstall(cs, Options{Prefix: prefix, DryRun: dry, Yes: yes}); err != nil {
		console.Errorf("uninstall: %v\n", err)
		return 1
	}
	return 0
}
