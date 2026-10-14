package components

// The install and uninstall commands. They live here, beside the catalog
// they drive and outside the host binary, so that `cmd/dcinstall` — the
// bootstrap scripts' entry point — runs the very same code without linking
// what the host links (Gusset). `devcouncil install` and `devcouncil
// uninstall` dispatch here; there is one implementation.

import (
	"fmt"
	"path/filepath"
	"strings"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/console"
)

func RunInstall(args []string) int {
	return runInstall(args, Options{})
}

// runInstall is RunInstall with the seams Options carries; tests fill them.
func runInstall(args []string, base Options) int {
	a, err := parseInstallArgs(args)
	if err != nil {
		console.Errorln(err)
		return 2
	}
	list, jsonOut, dry, names := a.list, a.jsonOut, a.dry, a.names
	if a.help {
		console.Print(HelpText)
		return 0
	}
	if a.strictConsumers && dry {
		// A rehearsal installs nothing, so there is no install for the
		// consumers to lag behind; refusing beats a verdict on nothing.
		console.Errorln("--strict-consumers judges a real install; --dry-run installs nothing")
		return 2
	}
	if list {
		prefix := Options{Prefix: a.prefix}.prefix()
		if jsonOut {
			b, err := ListJSON(prefix)
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
		console.Println()
		for _, line := range ReceiptLines(prefix) {
			console.Println(line)
		}
		return 0
	}
	cs, err := Resolve(names)
	if err != nil {
		console.Errorln(err)
		return 2
	}
	opts := base
	opts.Prefix, opts.DryRun, opts.Rev = a.prefix, dry, a.rev
	if a.src != "" {
		src, err := resolveSrc(a.src)
		if err != nil {
			console.Errorln(err)
			return 2
		}
		opts.SourceRoot = src
	}
	if IncludesHost(cs) && opts.source() == "" {
		console.Errorf("install: %v\n", opts.hostSourceError())
		return 2
	}
	if opts.Rev != "" && opts.source() != "" {
		console.Errorf("install: --rev pins a remote install, but this one builds the checkout at %s as it stands; check out that revision there instead\n", opts.source())
		return 2
	}
	if len(RustIDs(cs)) > 0 && opts.source() == "" {
		if _, err := opts.remoteRef(); err != nil {
			console.Errorf("install: %v\n", err)
			return 2
		}
	}
	var installErr error
	// A dry-run plan is already structured; do not print shell commands into JSON.
	if !jsonOut || !dry {
		installErr = Install(cs, opts)
	}
	var consumers ConsumerReport
	switch {
	case dry:
		consumers = notChecked("", "dry run: nothing was installed")
	case installErr != nil:
		consumers = notChecked("", "the install failed")
	default:
		consumers = CheckConsumers(opts.source(), a.gitpulse, opts)
	}
	if !jsonOut {
		PrintConsumerReport(consumers)
	}
	if jsonOut {
		ids := make([]string, 0, len(cs))
		for _, c := range cs {
			ids = append(ids, c.ID)
		}
		receipt := struct {
			OK         bool            `json:"ok"`
			DryRun     bool            `json:"dry_run"`
			Components []string        `json:"components"`
			Commands   []string        `json:"commands,omitempty"`
			Error      string          `json:"error,omitempty"`
			Consumers  *ConsumerReport `json:"consumers"`
		}{OK: installErr == nil, DryRun: dry, Components: ids, Consumers: &consumers}
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
	// The owner installs even when a consumer lags: refusing to install
	// itself would not bring the consumer up. --strict-consumers is for
	// callers that want the lag, or an unexamined consumer, to fail the run.
	if a.strictConsumers && !consumers.Passed() {
		console.Errorln("install: --strict-consumers: the schema consumers did not all report a match (see the consumers report)")
		return 1
	}
	return 0
}

// resolveSrc makes --src absolute and requires it to be a checkout; a
// symlinked directory is accepted (the checkout test follows it).
func resolveSrc(dir string) (string, error) {
	abs, err := filepath.Abs(dir)
	if err != nil {
		return "", fmt.Errorf("--src %s: %w", dir, err)
	}
	if !looksLikeCheckout(abs) {
		return "", notACheckoutError(dir)
	}
	return abs, nil
}

// installArgs is what `devcouncil install` was asked to do.
type installArgs struct {
	list, jsonOut, dry, help bool
	prefix, src              string
	// rev pins a remote install to a commit instead of the release tag.
	rev   string
	names []string
	// gitpulse is the GitPulse checkout whose schema-parity check runs after
	// the install; empty means GitPulseRootEnv, then the sibling checkout.
	gitpulse string
	// strictConsumers fails the install when a consumer lags or was not checked.
	strictConsumers bool
}

func parseInstallArgs(args []string) (installArgs, error) {
	var a installArgs
	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "-h", "--help":
			a.help = true
		case "--list":
			a.list = true
		case "--json":
			a.jsonOut = true
		case "--dry-run":
			a.dry = true
		case "--yes", "-y":
			// Confirmation is an uninstall question; the scripts this replaced
			// accepted it everywhere.
		case "--host-only":
			a.names = append(a.names, "host")
		case "--strict-consumers":
			a.strictConsumers = true
		case "--":
			a.names = append(a.names, args[i+1:]...)
			return a, nil
		case "--prefix":
			i++
			if i >= len(args) {
				return installArgs{}, fmt.Errorf("--prefix needs a directory")
			}
			a.prefix = args[i]
		case "--src":
			i++
			if i >= len(args) || args[i] == "" {
				return installArgs{}, fmt.Errorf("--src needs a directory")
			}
			a.src = args[i]
		case "--gitpulse":
			i++
			if i >= len(args) || args[i] == "" {
				return installArgs{}, fmt.Errorf("--gitpulse needs a directory")
			}
			a.gitpulse = args[i]
		case "--rev":
			i++
			if i >= len(args) || args[i] == "" {
				return installArgs{}, fmt.Errorf("--rev needs a commit")
			}
			a.rev = args[i]
		case "--only":
			// The bootstrap scripts' spelling of a positional name.
			i++
			if i >= len(args) || args[i] == "" {
				return installArgs{}, fmt.Errorf("--only needs a component name")
			}
			a.names = append(a.names, args[i])
		default:
			if strings.HasPrefix(args[i], "--only=") {
				only := strings.TrimPrefix(args[i], "--only=")
				if only == "" {
					return installArgs{}, fmt.Errorf("--only needs a component name")
				}
				a.names = append(a.names, only)
				continue
			}
			if strings.HasPrefix(args[i], "--prefix=") {
				a.prefix = strings.TrimPrefix(args[i], "--prefix=")
				if a.prefix == "" {
					return installArgs{}, fmt.Errorf("--prefix needs a directory")
				}
				continue
			}
			if strings.HasPrefix(args[i], "--src=") {
				a.src = strings.TrimPrefix(args[i], "--src=")
				if a.src == "" {
					return installArgs{}, fmt.Errorf("--src needs a directory")
				}
				continue
			}
			if strings.HasPrefix(args[i], "--rev=") {
				a.rev = strings.TrimPrefix(args[i], "--rev=")
				if a.rev == "" {
					return installArgs{}, fmt.Errorf("--rev needs a commit")
				}
				continue
			}
			if strings.HasPrefix(args[i], "--gitpulse=") {
				a.gitpulse = strings.TrimPrefix(args[i], "--gitpulse=")
				if a.gitpulse == "" {
					return installArgs{}, fmt.Errorf("--gitpulse needs a directory")
				}
				continue
			}
			if strings.HasPrefix(args[i], "-") {
				return installArgs{}, fmt.Errorf("unknown flag: %s", args[i])
			}
			a.names = append(a.names, args[i])
		}
	}
	return a, nil
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
