// Command dcinstall is `devcouncil install` and `devcouncil uninstall` for a
// machine that has no host yet. scripts/install.sh and scripts/install.ps1
// build it and run it with their arguments:
//
//	dcinstall install [names…] [options]
//	dcinstall uninstall [names…] [--all] [--yes] [options]
//
// It is the same code the host runs (devcouncil/components), built without
// what the host links: the host needs a Gusset checkout beside DevCouncil
// and its compiled engine, and a first install — `--only=devmap` above all —
// must not. Installing the host itself still builds it, and still needs both.
package main

import (
	"fmt"
	"os"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/components"
)

func run(args []string) int {
	if len(args) == 0 {
		return components.RunInstall(nil)
	}
	switch args[0] {
	case "install":
		return components.RunInstall(args[1:])
	case "uninstall":
		return components.RunUninstall(args[1:])
	}
	fmt.Fprintf(os.Stderr, "dcinstall: the first argument is install or uninstall, got %q\n", args[0])
	return 2
}

func main() {
	os.Exit(run(os.Args[1:]))
}
