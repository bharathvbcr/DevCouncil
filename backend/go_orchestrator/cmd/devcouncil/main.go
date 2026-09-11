// Command devcouncil is the DevCouncil host binary: MCP server, integrate, skills.
package main

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/integrate"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/mcp"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/skills"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/flags"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/gate"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
)

func main() {
	os.Exit(dispatch(os.Args[1:]))
}

func dispatch(args []string) int {
	if len(args) < 1 {
		usage()
		return 2
	}
	switch args[0] {
	case "mcp", "mcp-server":
		return runMCP()
	case "integrate", "integrations":
		return runIntegrate(args[1:])
	case "skills":
		return runSkills(args[1:])
	case "verify":
		return runVerify(args[1:])
	case "map", "graph":
		return runDevmap(mapArgs(args[1:]))
	case "ast":
		return runDevmap(astArgs(args[1:]))
	case "version", "--version", "-V":
		fmt.Println("devcouncil 0.1.0-phase7")
		return 0
	case "help", "-h", "--help":
		usage()
		return 0
	default:
		fmt.Fprintf(os.Stderr, "unknown command: %s\n", args[0])
		usage()
		return 2
	}
}

func usage() {
	fmt.Fprintf(os.Stderr, `devcouncil — DevCouncil host binary (Phase 7)

The same binary is installed as `+"`dev`"+` and `+"`devcouncil`"+`.

Usage:
  devcouncil mcp              Run the MCP stdio server
  devcouncil integrate HOST [--apply|--check|--dry-run] [--project-root DIR] [--write-gate]
  devcouncil integrations …        Alias of integrate
  devcouncil integrate uninstall --target hooks [--dry-run] [--project-root DIR]
  devcouncil skills list
  devcouncil skills scaffold [--skill NAME] [--project-root DIR] [--dry-run] [--check]
  devcouncil verify TASK_ID [--json] [--sandbox local|docker|nix] [--project-root DIR]
  devcouncil map [devmap args…]   Exec `+"`devmap`"+` (bare invocation: build --manifest)
  devcouncil graph …              Alias of map
  devcouncil ast …                Exec `+"`devmap ast`"+`

Hosts: %s
`, strings.Join(integrate.Hosts, ", "))
}

func projectRoot() string {
	if v := os.Getenv("DEVCOUNCIL_PROJECT_ROOT"); v != "" {
		return v
	}
	wd, err := os.Getwd()
	if err != nil {
		return "."
	}
	return wd
}

func runMCP() int {
	root := projectRoot()
	reg := openRegistry(root)
	srv := &mcp.Server{Registry: reg}
	if err := srv.Serve(); err != nil {
		fmt.Fprintf(os.Stderr, "devcouncil mcp: %v\n", err)
		return 1
	}
	return 0
}

func openRegistry(root string) *devcouncil.Registry {
	db := filepath.Join(root, ".devcouncil", "state.sqlite")
	var client *store.Client
	if _, err := os.Stat(db); err == nil {
		bin, _ := lookPath("dcstore")
		if bin == "" {
			bin = "dcstore"
		}
		client = store.New(bin, db)
	}
	var g *gate.Gate
	cfg := filepath.Join(root, ".devcouncil", "config.yaml")
	if regFlags, err := flags.NewHarnessRegistry(cfg); err == nil {
		if gg, err := gate.New(regFlags, root, nil); err == nil {
			g = gg
		}
	}
	return devcouncil.NewRegistry(root, client, g)
}

func lookPath(name string) (string, error) {
	return execLookPath(name)
}

// separated so tests can stub; default uses os/exec.
var execLookPath = func(name string) (string, error) {
	return lookPathImpl(name)
}

func lookPathImpl(name string) (string, error) {
	path := os.Getenv("PATH")
	for _, dir := range filepath.SplitList(path) {
		p := filepath.Join(dir, name)
		if st, err := os.Stat(p); err == nil && !st.IsDir() {
			return p, nil
		}
	}
	if home, err := os.UserHomeDir(); err == nil && home != "" {
		p := filepath.Join(home, ".local", "bin", name)
		if st, err := os.Stat(p); err == nil && !st.IsDir() {
			return p, nil
		}
	}
	return "", fmt.Errorf("%s not found", name)
}

func astArgs(rest []string) []string {
	out := make([]string, 0, 1+len(rest))
	out = append(out, "ast")
	return append(out, rest...)
}

// Kernel globals that clap accepts on either side of the subcommand. Value
// flags must be consumed here so `dev map --root /tmp` is not treated as a
// subcommand named `/tmp`.
var devmapBoolFlags = map[string]struct{}{
	"--json": {}, "--verbose": {}, "-v": {},
	"--help": {}, "-h": {}, "--version": {}, "-V": {},
}

var devmapValueFlags = map[string]struct{}{
	"--db": {}, "-d": {}, "--root": {}, "--progress": {},
}

func mapArgs(rest []string) []string {
	globals, tail, ok := peelDevmapGlobals(rest)
	if !ok {
		return rest
	}
	if helpOrVersion(globals) && !subcommandToken(tail) {
		return rest
	}
	if len(tail) == 0 {
		return concat(globals, "build", "--manifest")
	}
	if isFlagToken(tail[0]) {
		return concat(concat(globals, "build", "--manifest"), tail...)
	}
	return rest
}

func peelDevmapGlobals(args []string) (globals, rest []string, ok bool) {
	i := 0
	for i < len(args) {
		a := args[i]
		if a == "--" {
			return args[:i], args[i+1:], true
		}
		if name, val, eq := equalsFlag(a); eq {
			if _, isBool := devmapBoolFlags[name]; isBool {
				i++
				continue
			}
			if _, isVal := devmapValueFlags[name]; isVal {
				if val == "" {
					return nil, nil, false
				}
				i++
				continue
			}
			return args[:i], args[i:], true
		}
		if _, isBool := devmapBoolFlags[a]; isBool {
			i++
			continue
		}
		if _, isVal := devmapValueFlags[a]; isVal {
			if i+1 >= len(args) || isFlagToken(args[i+1]) {
				return nil, nil, false
			}
			i += 2
			continue
		}
		return args[:i], args[i:], true
	}
	return args, nil, true
}

func equalsFlag(a string) (name, val string, ok bool) {
	if !strings.HasPrefix(a, "--") {
		return "", "", false
	}
	i := strings.IndexByte(a, '=')
	if i < 0 {
		return "", "", false
	}
	return a[:i], a[i+1:], true
}

func isFlagToken(a string) bool {
	return strings.HasPrefix(a, "-") && a != "-"
}

func subcommandToken(tail []string) bool {
	for _, a := range tail {
		if !isFlagToken(a) && a != "--" {
			return true
		}
	}
	return false
}

func helpOrVersion(args []string) bool {
	for _, a := range args {
		name := a
		if i := strings.IndexByte(a, '='); i >= 0 {
			name = a[:i]
		}
		switch name {
		case "--help", "-h", "--version", "-V":
			return true
		}
	}
	return false
}

func concat(head []string, extra ...string) []string {
	out := make([]string, 0, len(head)+len(extra))
	out = append(out, head...)
	return append(out, extra...)
}

// A full `devmap build --manifest` can take minutes on a large tree; this is
// a hang bound, not a performance budget. Same duration as mapcli's delegated
// passthrough.
const devmapPassthroughTimeout = 10 * time.Minute

func runDevmap(args []string) int {
	bin, err := resolveDevmap()
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		return 1
	}
	ctx, cancel := context.WithTimeout(context.Background(), devmapPassthroughTimeout)
	defer cancel()
	cmd := exec.CommandContext(ctx, bin, args...)
	proc.ConfigureGroup(cmd)
	cmd.Stdin = os.Stdin
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	cmd.WaitDelay = 2 * time.Second
	runErr, timedOut := proc.RunBounded(ctx, cmd.Run)
	if timedOut {
		fmt.Fprintf(os.Stderr, "devmap did not return within %s\n", devmapPassthroughTimeout)
		return 1
	}
	if runErr != nil {
		if ee, ok := runErr.(*exec.ExitError); ok {
			return ee.ExitCode()
		}
		fmt.Fprintf(os.Stderr, "devcouncil: %v\n", runErr)
		return 1
	}
	return 0
}

// resolveDevmap honours DEVMAP_BIN when set: used or refused, never replaced
// by a PATH lookup. An empty value is unset.
func resolveDevmap() (string, error) {
	if bin := strings.TrimSpace(os.Getenv("DEVMAP_BIN")); bin != "" {
		info, err := os.Stat(bin)
		if err != nil {
			return "", fmt.Errorf("DEVMAP_BIN names %s: %w (an explicit override is used or refused, never replaced)", bin, err)
		}
		if info.IsDir() {
			return "", fmt.Errorf("DEVMAP_BIN names %s, which is a directory (an explicit override is used or refused, never replaced)", bin)
		}
		return bin, nil
	}
	found, err := lookPath("devmap")
	if err != nil {
		return "", fmt.Errorf("devmap binary not found on PATH or ~/.local/bin; build rust/ and install the `devmap` binary")
	}
	return found, nil
}

func runIntegrate(args []string) int {
	if len(args) < 1 {
		fmt.Fprintln(os.Stderr, "integrate requires a host")
		return 2
	}
	if args[0] == "uninstall" {
		return runIntegrateUninstall(args[1:])
	}
	opts := integrate.Options{Root: projectRoot(), Host: args[0], Mode: integrate.ModeCheck}
	for i := 1; i < len(args); i++ {
		switch args[i] {
		case "--apply":
			opts.Mode = integrate.ModeApply
		case "--check":
			opts.Mode = integrate.ModeCheck
		case "--dry-run":
			opts.Mode = integrate.ModeDryRun
		case "--write-gate":
			opts.WriteGate = true
		case "--project-root":
			i++
			if i >= len(args) {
				fmt.Fprintln(os.Stderr, "--project-root needs a value")
				return 2
			}
			opts.Root = args[i]
		case "--json":
			// always print receipt JSON on stdout for scripting
		default:
			fmt.Fprintf(os.Stderr, "unknown flag: %s\n", args[i])
			return 2
		}
	}
	self, _ := os.Executable()
	opts.SelfBin = self
	receipt, err := integrate.Run(opts)
	if err != nil {
		fmt.Fprintf(os.Stderr, "integrate: %v\n", err)
		enc := json.NewEncoder(os.Stdout)
		enc.SetIndent("", "  ")
		_ = enc.Encode(receipt)
		return 1
	}
	enc := json.NewEncoder(os.Stdout)
	enc.SetIndent("", "  ")
	_ = enc.Encode(receipt)
	if opts.Mode == integrate.ModeApply {
		fmt.Fprintf(os.Stderr, "%s integration configured (%s).\n", opts.Host, modeLabel(opts))
	}
	return 0
}

// runIntegrateUninstall removes host wiring. `--target hooks` is what
// `dev hook disable` calls: DevCouncil lifecycle hooks are retired, and this is
// the one command that takes their registrations back off a host.
func runIntegrateUninstall(args []string) int {
	opts := integrate.UninstallOptions{Root: projectRoot(), Target: integrate.TargetHooks, Mode: integrate.ModeApply}
	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "--target":
			i++
			if i >= len(args) {
				fmt.Fprintln(os.Stderr, "--target needs a value")
				return 2
			}
			opts.Target = integrate.Target(args[i])
		case "--apply":
			opts.Mode = integrate.ModeApply
		case "--dry-run", "--check":
			opts.Mode = integrate.ModeDryRun
		case "--project-root":
			i++
			if i >= len(args) {
				fmt.Fprintln(os.Stderr, "--project-root needs a value")
				return 2
			}
			opts.Root = args[i]
		case "--json":
			// receipt JSON always goes to stdout
		default:
			fmt.Fprintf(os.Stderr, "unknown flag: %s\n", args[i])
			return 2
		}
	}
	receipt, err := integrate.Uninstall(opts)
	enc := json.NewEncoder(os.Stdout)
	enc.SetIndent("", "  ")
	if err != nil {
		fmt.Fprintf(os.Stderr, "integrate uninstall: %v\n", err)
		if receipt != nil {
			_ = enc.Encode(receipt)
		}
		return 1
	}
	_ = enc.Encode(receipt)
	return 0
}

func modeLabel(opts integrate.Options) string {
	if opts.WriteGate {
		return "containment mode (write-gate)"
	}
	return "assist mode (no write-gate)"
}

func runSkills(args []string) int {
	if len(args) < 1 {
		fmt.Fprintln(os.Stderr, "usage: devcouncil skills list|scaffold ...")
		return 2
	}
	switch args[0] {
	case "list":
		return runSkillsList(args[1:])
	case "scaffold":
		return runSkillsScaffold(args[1:])
	default:
		fmt.Fprintf(os.Stderr, "unknown skills subcommand: %s\n", args[0])
		fmt.Fprintln(os.Stderr, "usage: devcouncil skills list|scaffold ...")
		return 2
	}
}

func runSkillsList(args []string) int {
	jsonOut := false
	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "--json":
			jsonOut = true
		default:
			fmt.Fprintf(os.Stderr, "unknown flag: %s\n", args[i])
			return 2
		}
	}
	all, err := skills.Embedded.Load()
	if err != nil {
		fmt.Fprintf(os.Stderr, "skills: %v\n", err)
		return 1
	}
	if jsonOut {
		names := make([]string, 0, len(all))
		for _, s := range all {
			names = append(names, s.Name)
		}
		enc := json.NewEncoder(os.Stdout)
		enc.SetIndent("", "  ")
		_ = enc.Encode(map[string]any{"skills": names, "total": len(names)})
		return 0
	}
	for _, s := range all {
		fmt.Println(s.Name)
	}
	return 0
}

func runSkillsScaffold(args []string) int {
	root := projectRoot()
	dryRun := false
	checkOnly := false
	var filter []string
	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "--dry-run":
			dryRun = true
		case "--check":
			checkOnly = true
		case "--project-root":
			i++
			if i >= len(args) {
				return 2
			}
			root = args[i]
		case "--skill":
			i++
			if i >= len(args) {
				return 2
			}
			filter = append(filter, args[i])
		default:
			fmt.Fprintf(os.Stderr, "unknown flag: %s\n", args[i])
			return 2
		}
	}
	all, err := skills.Embedded.Load()
	if err != nil {
		fmt.Fprintf(os.Stderr, "skills: %v\n", err)
		return 1
	}
	selected := all
	if len(filter) > 0 {
		want := map[string]struct{}{}
		for _, n := range filter {
			want[n] = struct{}{}
		}
		selected = nil
		for _, s := range all {
			if _, ok := want[s.Name]; ok {
				selected = append(selected, s)
			}
		}
		if len(selected) == 0 {
			fmt.Fprintf(os.Stderr, "no matching skills for %v\n", filter)
			return 1
		}
	}
	result, err := skills.Scaffold(skills.Options{
		Root: root, Skills: selected, DryRun: dryRun, CheckOnly: checkOnly,
	})
	if err != nil {
		fmt.Fprintf(os.Stderr, "skills scaffold: %v\n", err)
		return 1
	}
	if dryRun || checkOnly {
		enc := json.NewEncoder(os.Stdout)
		enc.SetIndent("", "  ")
		_ = enc.Encode(result)
		return 0
	}
	fmt.Printf("Wrote %d skill file(s):\n", len(result.Files))
	for _, f := range result.Files {
		fmt.Printf("  %s\n", f)
	}
	return 0
}

func runVerify(args []string) int {
	root := projectRoot()
	jsonOut := false
	sandbox := "local"
	taskID := ""
	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "--json":
			jsonOut = true
		case "--sandbox":
			i++
			if i >= len(args) {
				fmt.Fprintln(os.Stderr, "--sandbox needs a value")
				return 2
			}
			sandbox = args[i]
		case "--project-root":
			i++
			if i >= len(args) {
				fmt.Fprintln(os.Stderr, "--project-root needs a value")
				return 2
			}
			root = args[i]
		case "-h", "--help":
			fmt.Fprintln(os.Stderr, "usage: devcouncil verify TASK_ID [--json] [--sandbox local]")
			return 0
		default:
			if strings.HasPrefix(args[i], "-") {
				fmt.Fprintf(os.Stderr, "unknown flag: %s\n", args[i])
				return 2
			}
			if taskID == "" {
				taskID = args[i]
			} else {
				fmt.Fprintf(os.Stderr, "unexpected argument: %s\n", args[i])
				return 2
			}
		}
	}
	if taskID == "" {
		fmt.Fprintln(os.Stderr, "verify requires TASK_ID")
		return 2
	}
	reg := openRegistry(root)
	gateMode := "enforce"
	if reg.Lease != nil && reg.Lease.GateMode != "" {
		gateMode = reg.Lease.GateMode
	}
	return verify.RunCLI(context.Background(), root, reg.Store, taskID, gateMode, sandbox, jsonOut)
}
