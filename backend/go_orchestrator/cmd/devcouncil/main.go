// Command devcouncil is the DevCouncil host binary: MCP server, integrate, skills.
package main

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"os/signal"
	"path/filepath"
	"strings"
	"syscall"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/console"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/gatescfg"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/integrate"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/mcp"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/skills"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/flags"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/gate"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/policy"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/repomap"
)

func main() {
	os.Exit(runCLI(os.Args[1:]))
}

func isVersionArg(arg string) bool {
	return arg == "version" || arg == "--version" || arg == "-V" || arg == "-v"
}

func dispatch(args []string) int {
	if len(args) < 1 {
		usage()
		return 2
	}
	if args[0] == "--json" && len(args) > 1 && isVersionArg(args[1]) {
		if _, err := console.Println(`{"ok":true,"id":"host","component":"devcouncil","version":"` + Version + `"}`); err != nil {
			console.Errorf("version: %v\n", err)
			return 1
		}
		return 0
	}
	run, ok := commands[args[0]]
	if !ok {
		console.Errorf("unknown command: %s\n", args[0])
		usage()
		return 2
	}
	return run(args[1:])
}

// commands is the dispatch table: every top-level command this binary runs,
// keyed by the name a caller types after `dev` or `devcouncil`.
//
// It is data rather than a switch so that the things which name commands to
// agents — the policy allowlist, its denial text and the packaged skills — can
// be held to it by a test. Each of those once named `dev status`,
// `dev checkout` or `dev doctor`, which exit 2 here; a name that is not a key
// in this table is a name no agent can run.
var commands = map[string]func(args []string) int{
	"mcp":          func([]string) int { return runMCP() },
	"mcp-server":   func([]string) int { return runMCP() },
	"integrate":    runIntegrate,
	"integrations": runIntegrate,
	"skills":       runSkills,
	"verify":       runVerify,
	"grep":         runGrep,
	"install":      runInstall,
	"uninstall":    runUninstall,
	"disable":      runDisable,
	"enable":       runEnable,
	"gate":         runGate,
	"hook":         runHook,
	"map":          func(args []string) int { return runDevmap(mapArgs(args)) },
	"graph":        func(args []string) int { return runDevmap(mapArgs(args)) },
	"ast":          func(args []string) int { return runDevmap(astArgs(args)) },
	"gusset-check": func([]string) int { return runGussetCheck() },
	"version":      runVersion,
	"--version":    runVersion,
	"-V":           runVersion,
	"-v":           runVersion,
	"help":         runHelp,
	"-h":           runHelp,
	"--help":       runHelp,
}

func runVersion(args []string) int {
	for _, arg := range args {
		if arg == "--json" {
			console.Println(`{"ok":true,"id":"host","component":"devcouncil","version":"` + Version + `"}`)
			return 0
		}
	}
	console.Println("devcouncil " + Version)
	return 0
}

func runHelp([]string) int {
	usage()
	return 0
}

func usage() {
	console.Errorf(`devcouncil — DevCouncil host binary (Phase 7)

The same binary is installed as ` + "`dev`" + ` and ` + "`devcouncil`" + `.

Usage:
  devcouncil mcp              Run the MCP stdio server
  devcouncil install [names…] [--list] [--json] [--prefix DIR] [--dry-run]
  devcouncil uninstall [names…] [--yes] [--prefix DIR]
  devcouncil disable NAME [--prefix DIR]
  devcouncil enable NAME [--prefix DIR]
  devcouncil gate status [--json] [--project-root DIR]
  devcouncil gate set --mode off|advisory|enforce
  devcouncil hook <event>     Retired lifecycle compatibility (silent no-op)
  devcouncil hook status|disable [--project-root DIR] [--client HOST]
  devcouncil integrate HOST [--apply|--check|--dry-run] [--project-root DIR] [--devmap-bin PATH]
  devcouncil integrations …        Alias of integrate
  devcouncil integrate uninstall --target hooks [--dry-run] [--project-root DIR]
  devcouncil skills list
  devcouncil skills scaffold [--skill NAME] [--project-root DIR] [--dry-run] [--check]
                                   --check writes nothing and exits 1 if any file is missing or differs
  devcouncil verify TASK_ID [--json] [--mode off|advisory|enforce] [--sandbox local] [--coverage PATH]
  devcouncil grep PATTERN [--json] [--path DIR] [--max N] [--ignore-case]
                                   Search via the ` + "`dcgrep`" + ` client (missing binary is an error, never an empty match list)
  devcouncil map [devmap args…]   Exec ` + "`devmap`" + ` (bare invocation: build --manifest)
  devcouncil graph …              Alias of map
  devcouncil ast …                Exec ` + "`devmap ast`" + `
  devcouncil gusset-check         Prove the linked Rust engine is dc-glob on Gusset

Presentation: --progress auto|always|never (default: auto). JSON stays on stdout.
NO_COLOR removes color; TERM=dumb and non-UTF-8 locales use simpler output.

First-time / standalone (no host yet):
  bash scripts/install.sh --only=devmap
  bash scripts/install.sh --help

Hosts: the ones ` + "`devmap integrate --help`" + ` lists; DevMap owns the host table.
`)
}

// projectRoot is the repository a command acts on when no --project-root was
// given: DEVCOUNCIL_PROJECT_ROOT, else the working directory.
//
// Both are checked by checkProjectRoot. The environment value used to be
// returned verbatim, and a host that wrote `"${CLAUDE_PROJECT_DIR}"` into an
// MCP config without expanding it handed this binary a relative path whose
// first component is a literal `${CLAUDE_PROJECT_DIR}` — so state was created
// under a directory of that name beside wherever the host happened to start.
// A root that is wrong is refused before anything is read or written under it.
func projectRoot() (string, error) {
	if v, set := os.LookupEnv("DEVCOUNCIL_PROJECT_ROOT"); set {
		return checkProjectRoot(v, "DEVCOUNCIL_PROJECT_ROOT")
	}
	wd, err := os.Getwd()
	if err != nil {
		return "", fmt.Errorf("cannot resolve the project root: the working directory is unreadable (%v); pass --project-root DIR", err)
	}
	return checkProjectRoot(wd, "the working directory")
}

// resolveRoot is the root after flag parsing: the --project-root value when one
// was given (already checked by checkProjectRoot), otherwise projectRoot().
func resolveRoot(fromFlag string) (string, error) {
	if fromFlag != "" {
		return fromFlag, nil
	}
	return projectRoot()
}

// checkProjectRoot refuses a project root that cannot be the repository the
// caller meant, and returns it cleaned. source names where the value came from
// so the refusal says what to fix.
//
// Refused: empty; containing `$` (an unexpanded variable — DevMap refuses the
// same class in rust/devmap-serve/src/root_resolve.rs); relative (it would
// resolve against whatever directory the host launched from); missing; not a
// directory. Nothing is created to make a root valid.
func checkProjectRoot(dir, source string) (string, error) {
	if strings.TrimSpace(dir) == "" {
		return "", fmt.Errorf("%s is empty; it must name the repository directory", source)
	}
	if strings.Contains(dir, "$") {
		return "", fmt.Errorf("%s is %q, which contains an unexpanded shell variable: "+
			"the host that launched this command did not substitute it. Configure the host "+
			"with the absolute repository path", source, dir)
	}
	if !filepath.IsAbs(dir) {
		return "", fmt.Errorf("%s is %q, which is relative; it must be an absolute path to the repository", source, dir)
	}
	info, err := os.Stat(dir)
	if err != nil {
		return "", fmt.Errorf("%s is %q, which cannot be used: %w", source, dir, err)
	}
	if !info.IsDir() {
		return "", fmt.Errorf("%s is %q, which is not a directory", source, dir)
	}
	return filepath.Clean(dir), nil
}

// projectRootFlag reads the value after a --project-root at args[i], checks it,
// and reports it on stderr when it is refused. ok is false for a missing or
// refused value; the caller exits 2.
func projectRootFlag(args []string, i int) (root string, ok bool) {
	if i >= len(args) {
		console.Errorln("--project-root needs a directory")
		return "", false
	}
	root, err := checkProjectRoot(args[i], "--project-root")
	if err != nil {
		console.Errorln(err)
		return "", false
	}
	return root, true
}

func runMCP() int {
	root, err := projectRoot()
	if err != nil {
		console.Errorf("devcouncil mcp: %v\n", err)
		return 2
	}
	g, err := openGate(root)
	if err != nil {
		// Served without a gate rather than not at all: the task loop and the
		// verifier do not depend on it, and devcouncil_policy_check_write
		// answers not_initialized instead of a verdict it could not reach.
		console.Errorf("devcouncil mcp: write gate unavailable: %v\n", err)
	}
	reg := devcouncil.NewRegistry(root, openStore(root), g)
	srv := &mcp.Server{Registry: reg}
	if err := srv.Serve(); err != nil {
		console.Errorf("devcouncil mcp: %v\n", err)
		return 1
	}
	return 0
}

// openStore returns the task-store client, or nil when the project has no
// state database.
func openStore(root string) *store.Client {
	db := filepath.Join(root, ".devcouncil", "state.sqlite")
	if _, err := os.Stat(db); err != nil {
		return nil
	}
	bin, _ := lookPath("dcstore")
	if bin == "" {
		bin = "dcstore"
	}
	return store.New(bin, db)
}

// openGate builds the write gate the MCP policy tool consults, with the
// repository's code graph behind its subsystem-neighbour rung.
//
// The graph is read from DevMap's resolved state directory (repomap.CodeGraphPath),
// the same file Manvi's buildGate reads. It used to be nil here, so every
// unplanned write recorded `repo_map.unavailable` and fell back to the
// same-directory rung even in a repository with a built index.
//
// A graph that is absent leaves the map nil, which the policy layer records as
// `repo_map.unavailable` on each decision it would have consulted. A graph that
// is present but unreadable does the same and is also reported here, because
// "never built" and "built and damaged" send an operator to different places.
// Neither case allows anything the map would have refused: without a map the
// rung falls back to same-directory, which is narrower than the neighbour
// relation, never wider.
func openGate(root string) (*gate.Gate, error) {
	regFlags, err := flags.NewHarnessRegistry(filepath.Join(root, ".devcouncil", "config.yaml"))
	if err != nil {
		return nil, err
	}
	graphPath := repomap.CodeGraphPath(root)
	m, err := repomap.LoadIfPresent(graphPath)
	if err != nil {
		console.Errorf("devcouncil: repo map %s is unreadable (%v); the write gate's "+
			"subsystem-neighbour rung is unavailable and unplanned writes fall back to "+
			"same-directory scope. Rebuild it with `dev map`.\n", graphPath, err)
		m = nil
	}
	// Assigned through the nil check, as Manvi's buildGate does: a nil
	// *repomap.Map stored in the policy.SubsystemMap interface is a non-nil
	// interface, and the policy layer would consult a map that answers nothing
	// instead of recording that it has none.
	var subsystems policy.SubsystemMap
	if m != nil {
		subsystems = m
	}
	g, err := gate.New(regFlags, root, subsystems)
	if err != nil {
		return nil, err
	}
	// The engine answers every pattern question this gate asks; a failure is a
	// hard denial under the engine rules, not a guess.
	g.Matcher = policyMatcher()
	return g, nil
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
		case "--help", "-h", "--version", "-V", "-v":
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
// a hang bound, not a performance budget.
const devmapPassthroughTimeout = 10 * time.Minute

func runDevmap(args []string) int {
	bin, err := resolveDevmap()
	if err != nil {
		console.Errorln(err)
		return 1
	}
	parent, stop := signal.NotifyContext(console.Context(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	ctx, cancel := context.WithTimeout(parent, devmapPassthroughTimeout)
	defer cancel()
	cmd := exec.CommandContext(ctx, bin, args...)
	proc.ConfigureGroup(cmd)
	cmd.Stdin = os.Stdin
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	cmd.WaitDelay = 2 * time.Second
	runErr, timedOut := proc.RunBoundedWithCleanup(ctx, cmd.Run)
	if timedOut {
		if ctx.Err() == context.Canceled {
			console.Errorln("DevMap command cancelled")
			return 130
		}
		console.Errorf("devmap did not return within %s\n", devmapPassthroughTimeout)
		return 1
	}
	if runErr != nil {
		if ee, ok := runErr.(*exec.ExitError); ok {
			return ee.ExitCode()
		}
		console.Errorf("devcouncil: %v\n", runErr)
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
		// `devmap integrate` owns the host list, and its own refusal of an
		// unknown name prints it.
		console.Errorln("integrate requires a host; `devmap integrate --help` lists them")
		return 2
	}
	if args[0] == "uninstall" {
		return runIntegrateUninstall(args[1:])
	}
	opts := integrate.Options{Host: args[0], Mode: integrate.ModeCheck}
	for i := 1; i < len(args); i++ {
		switch args[i] {
		case "--apply":
			opts.Mode = integrate.ModeApply
		case "--check":
			opts.Mode = integrate.ModeCheck
		case "--dry-run":
			opts.Mode = integrate.ModeDryRun
		case "--write-gate":
			// Retired, not merely unsupported. Answered by name rather than
			// left to "unknown flag" because it appears in existing scripts
			// and release notes, and the reason it went away is the part a
			// caller needs: nothing here ever gated a write.
			console.Errorln("--write-gate was removed: DevCouncil lifecycle hooks are " +
				"retired, so no host hook can gate a write. Nothing enforced it before " +
				"this flag was removed either — it was refused. Use the devcouncil_* MCP " +
				"policy tools and verification explicitly.")
			return 2
		case "--project-root":
			i++
			root, ok := projectRootFlag(args, i)
			if !ok {
				return 2
			}
			opts.Root = root
		case "--devmap-bin":
			i++
			if i >= len(args) {
				console.Errorln("--devmap-bin needs a path")
				return 2
			}
			opts.DevmapBin = args[i]
		case "--json":
			// always print receipt JSON on stdout for scripting
		default:
			console.Errorf("unknown flag: %s\n", args[i])
			return 2
		}
	}
	root, err := resolveRoot(opts.Root)
	if err != nil {
		console.Errorf("integrate: %v\n", err)
		return 2
	}
	opts.Root = root
	self, _ := os.Executable()
	opts.SelfBin = self
	receipt, err := integrate.Run(opts)
	if err != nil {
		console.Errorf("integrate: %v\n", err)
		// A partial failure still carries what was written before it stopped,
		// and that receipt is the only record of it. A refusal carries no
		// receipt at all, and emitting `null` there reads as a result.
		if receipt != nil {
			if writeErr := console.JSON(receipt); writeErr != nil {
				console.Errorln(writeErr)
			}
		}
		return 1
	}
	if err := console.JSON(receipt); err != nil {
		console.Errorln(err)
		return 1
	}
	if opts.Mode == integrate.ModeApply {
		console.Errorf("%s integration configured (host hooks retired).\n", opts.Host)
	}
	return 0
}

// runIntegrateUninstall removes host wiring. `--target hooks` is what
// `dev hook disable` calls: DevCouncil lifecycle hooks are retired, and this is
// the one command that takes their registrations back off a host.
func runIntegrateUninstall(args []string) int {
	opts := integrate.UninstallOptions{Target: integrate.TargetHooks, Mode: integrate.ModeApply}
	modeSet := false
	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "--apply", "--dry-run", "--check":
			mode := integrate.Mode(strings.TrimPrefix(args[i], "--"))
			if modeSet && opts.Mode != mode {
				console.Errorln("conflicting cleanup modes")
				return 2
			}
			opts.Mode = mode
			modeSet = true
		case "--target", "--project-root", "--client":
			flag := args[i]
			i++
			if i >= len(args) || args[i] == "" || strings.HasPrefix(args[i], "--") {
				console.Errorf("%s needs a value\n", flag)
				return 2
			}
			switch flag {
			case "--target":
				opts.Target = integrate.Target(args[i])
			case "--project-root":
				root, ok := projectRootFlag(args, i)
				if !ok {
					return 2
				}
				opts.Root = root
			case "--client":
				opts.Client = args[i]
			}
		case "--json":
		case "--help", "-h":
			return printHookHelp()
		default:
			console.Errorf("unknown cleanup flag: %s\n", args[i])
			return 2
		}
	}
	root, err := resolveRoot(opts.Root)
	if err != nil {
		console.Errorf("integrate uninstall: %v\n", err)
		return 2
	}
	opts.Root = root
	receipt, err := integrate.Uninstall(opts)
	if receipt != nil {
		if writeErr := console.JSON(receipt); writeErr != nil {
			console.Errorf("receipt: %v\n", writeErr)
			return 1
		}
	}
	if err != nil {
		console.Errorf("integrate uninstall: %v\n", err)
		return 1
	}
	if opts.Mode == integrate.ModeCheck && len(receipt.HookEntries) > 0 {
		return 1
	}
	return 0
}

func runSkills(args []string) int {
	if len(args) < 1 {
		console.Errorln("usage: devcouncil skills list|scaffold ...")
		return 2
	}
	switch args[0] {
	case "list":
		return runSkillsList(args[1:])
	case "scaffold":
		return runSkillsScaffold(args[1:])
	default:
		console.Errorf("unknown skills subcommand: %s\n", args[0])
		console.Errorln("usage: devcouncil skills list|scaffold ...")
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
			console.Errorf("unknown flag: %s\n", args[i])
			return 2
		}
	}
	all, err := skills.Embedded.Load()
	if err != nil {
		console.Errorf("skills: %v\n", err)
		return 1
	}
	if jsonOut {
		names := make([]string, 0, len(all))
		for _, s := range all {
			names = append(names, s.Name)
		}
		if err := console.JSON(map[string]any{"skills": names, "total": len(names)}); err != nil {
			console.Errorln(err)
			return 1
		}
		return 0
	}
	for _, s := range all {
		console.Println(s.Name)
	}
	return 0
}

func runSkillsScaffold(args []string) int {
	root := ""
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
			r, ok := projectRootFlag(args, i)
			if !ok {
				return 2
			}
			root = r
		case "--skill":
			i++
			if i >= len(args) {
				return 2
			}
			filter = append(filter, args[i])
		default:
			console.Errorf("unknown flag: %s\n", args[i])
			return 2
		}
	}
	root, err := resolveRoot(root)
	if err != nil {
		console.Errorf("skills scaffold: %v\n", err)
		return 2
	}
	all, err := skills.Embedded.Load()
	if err != nil {
		console.Errorf("skills: %v\n", err)
		return 1
	}
	selected := all
	if len(filter) > 0 {
		have := map[string]skills.Skill{}
		for _, s := range all {
			have[s.Name] = s
		}
		// Every requested name must exist. Installing the subset that happened
		// to match and exiting 0 reports a typo as a completed install.
		selected = nil
		seen := map[string]struct{}{}
		for _, name := range filter {
			s, ok := have[name]
			if !ok {
				console.Errorf("Unknown skill: %s\n", name)
				return 1
			}
			if _, repeat := seen[name]; repeat {
				continue
			}
			seen[name] = struct{}{}
			selected = append(selected, s)
		}
	}
	result, err := skills.Scaffold(skills.Options{
		Root: root, Skills: selected, DryRun: dryRun, CheckOnly: checkOnly,
	})
	if err != nil {
		console.Errorf("skills scaffold: %v\n", err)
		return 1
	}
	if dryRun || checkOnly {
		if err := console.JSON(result); err != nil {
			console.Errorln(err)
			return 1
		}
		// --check answers a question, so it must be able to say no: a tree that
		// still needs files exits non-zero, an installed one exits 0.
		if checkOnly && len(result.Files) > 0 {
			return 1
		}
		return 0
	}
	console.Printf("Wrote %d skill file(s):\n", len(result.Files))
	for _, f := range result.Files {
		console.Printf("  %s\n", f)
	}
	return 0
}

func runVerify(args []string) int {
	root := ""
	jsonOut := false
	sandbox := verify.SandboxLocal
	taskID := ""
	modeFlag := ""
	coveragePath := ""
	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "--json":
			jsonOut = true
		case "--coverage":
			i++
			if i >= len(args) {
				console.Errorln("--coverage needs a path to a coverage profile")
				return 2
			}
			coveragePath = args[i]
		case "--sandbox":
			i++
			if i >= len(args) {
				console.Errorln("--sandbox needs a value")
				return 2
			}
			parsed, err := verify.ParseSandbox(args[i])
			if err != nil {
				console.Errorln(err)
				return 2
			}
			sandbox = parsed
		case "--mode":
			i++
			if i >= len(args) {
				console.Errorln("--mode needs off, advisory, or enforce")
				return 2
			}
			modeFlag = args[i]
		case "--project-root":
			i++
			r, ok := projectRootFlag(args, i)
			if !ok {
				return 2
			}
			root = r
		case "-h", "--help":
			console.Errorln("usage: devcouncil verify TASK_ID [--json] [--mode off|advisory|enforce] " +
				"[--sandbox local] [--coverage PATH]")
			return 0
		default:
			if strings.HasPrefix(args[i], "-") {
				console.Errorf("unknown flag: %s\n", args[i])
				return 2
			}
			if taskID == "" {
				taskID = args[i]
			} else {
				console.Errorf("unexpected argument: %s\n", args[i])
				return 2
			}
		}
	}
	if taskID == "" {
		console.Errorln("verify requires TASK_ID")
		return 2
	}
	root, err := resolveRoot(root)
	if err != nil {
		console.Errorf("verify: %v\n", err)
		return 2
	}
	// No write gate: verification reads the store and the lease, and building
	// the gate would load the whole code graph for a run that never asks it a
	// question.
	reg := devcouncil.NewRegistry(root, openStore(root), nil)
	gateMode := ""
	if reg.Lease != nil {
		gateMode = reg.Lease.GateMode
	}
	if modeFlag != "" {
		parsed, err := gatescfg.ParseMode(modeFlag)
		if err != nil {
			console.Errorln(err)
			return 2
		}
		gateMode = parsed
	}
	if gateMode == "" {
		gateMode = gatescfg.Load(root).VerificationMode
	}
	return verify.RunCLI(console.Context(), root, reg.Store, taskID, gateMode, sandbox, coveragePath, jsonOut)
}
