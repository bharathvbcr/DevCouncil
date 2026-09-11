// Package mapcli is the Go port of DevCouncil's `dev map` command surface.
//
// The Python original (`cli/commands/map.py` plus the 32 subcommands mounted
// from `cli/commands/graph_cmd.py`) is a shell over the Rust `devmap` kernel.
// Eleven of its 35 subcommands — search, trace, dead, explore, affected,
// preview, workspace, savings, clones, impact, status — already exist natively
// in that binary with their own `--json`, so this package delegates those
// rather than reimplementing them. What it ports is the part the kernel does
// not have: binary discovery, DevCouncil's project-root conventions, and the
// human/JSON presentation layer.
//
// # The output contract, made structural
//
// DevCouncil's `--json` contract is: exactly one JSON object on stdout, every
// diagnostic on stderr. The Python implementation expressed that as a
// convention — each call site chose a stream — and the convention broke:
// `dev campaign run --json` printed a dry-run banner to stdout ahead of its
// payload, and two tests had to slice past it to parse anything.
//
// A command here cannot make that mistake, because a command here never sees
// stdout. It returns a payload; [Run] serializes it. Diagnostics go to
// [Env.Stderr], which is the only writer a command is handed. The contract is
// therefore a property of the types rather than a rule contributors must
// remember.
package mapcli

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"sort"
	"strings"
)

// Env is everything a command may write to or read about its surroundings.
//
// Note what is absent: stdout. The payload sink belongs to [Run] alone, so no
// command can put non-JSON on it while --json is set.
type Env struct {
	// Stderr receives every human-facing diagnostic — progress, warnings,
	// banners — in both output modes. It is never the payload sink.
	Stderr io.Writer
	// Root is the repository the map describes.
	Root string
	// JSON selects the machine-readable surface.
	JSON bool
	// Binary is the resolved `devmap` kernel. Empty means "discover it".
	Binary string
}

// Diagf writes one diagnostic line. It exists so commands have an obvious
// right way to say something to a human without reaching for a stream.
func (e *Env) Diagf(format string, args ...any) {
	if e.Stderr == nil {
		return
	}
	fmt.Fprintf(e.Stderr, format+"\n", args...)
}

// Payload is what a command returns: a value that can serialize itself as JSON
// (by being any serializable Go value) and, when it can do better than a JSON
// dump for a person, render itself.
type Payload any

// HumanRenderer is the optional half of [Payload]. A payload that implements it
// controls its own non-JSON presentation; one that does not is shown as
// indented JSON, which is a poor table but never a wrong one.
type HumanRenderer interface {
	Human(w io.Writer)
}

// Command is one `dcmap` subcommand.
//
// Returning `(payload, error)` rather than writing output is the whole point:
// it is what lets [Run] guarantee the stdout contract for every command at
// once, including ones not yet written.
type Command struct {
	Name    string
	Summary string
	// Run performs the command. args excludes the subcommand name.
	Run func(ctx context.Context, env *Env, args []string) (Payload, error)
	// Delegated marks a subcommand the Rust kernel implements natively. These
	// are passed through rather than ported; see [passthrough].
	Delegated bool
}

// errorPayload is what --json emits when a command fails.
//
// A failing --json invocation still owes its caller exactly one JSON object.
// Emitting nothing — which the Python `dev campaign run --json` did when no
// plan existed — leaves an agent unable to tell a refusal from a crash.
type errorPayload struct {
	OK    bool   `json:"ok"`
	Error string `json:"error"`
}

// Run dispatches one invocation and is the only writer of stdout.
//
// It returns the process exit code rather than calling os.Exit, so that tests
// drive it directly on buffers.
func Run(ctx context.Context, env *Env, stdout io.Writer, args []string) int {
	if len(args) == 0 || args[0] == "help" || args[0] == "-h" || args[0] == "--help" {
		usage(env.Stderr)
		// Asking for help is not a failure, but help is a diagnostic: it goes
		// to stderr, and stdout stays empty rather than carrying prose that a
		// --json caller would try to parse.
		return 0
	}

	name := args[0]
	cmd, ok := lookup(name)
	if !ok {
		env.Diagf("unknown subcommand %q", name)
		usage(env.Stderr)
		return emit(env, stdout, errorPayload{OK: false, Error: "unknown subcommand " + name}, 2)
	}

	payload, err := cmd.Run(ctx, env, args[1:])
	if err != nil {
		env.Diagf("%s: %v", name, err)
		return emit(env, stdout, errorPayload{OK: false, Error: err.Error()}, 1)
	}
	return emit(env, stdout, payload, 0)
}

// emit writes the single stdout artifact for this invocation and returns code.
//
// A nil payload writes nothing: a delegated subcommand has already streamed the
// kernel's own bytes to stdout, and appending a second object to them would
// break the very contract this function exists to keep.
func emit(env *Env, stdout io.Writer, payload Payload, code int) int {
	if payload == nil {
		return code
	}
	if env.JSON {
		enc := json.NewEncoder(stdout)
		enc.SetIndent("", "  ")
		if err := enc.Encode(payload); err != nil {
			// The payload is unserializable, so there is no object to emit and
			// stdout must stay empty rather than hold a fragment.
			env.Diagf("cannot serialize result: %v", err)
			return 1
		}
		return code
	}
	if r, ok := payload.(HumanRenderer); ok {
		r.Human(stdout)
		return code
	}
	enc := json.NewEncoder(stdout)
	enc.SetIndent("", "  ")
	if err := enc.Encode(payload); err != nil {
		env.Diagf("cannot render result: %v", err)
		return 1
	}
	return code
}

// registry is the subcommand table. Delegated entries name what the kernel
// already implements; the rest are ported here.
var registry = buildRegistry()

func lookup(name string) (Command, bool) {
	for _, c := range registry {
		if c.Name == name {
			return c, true
		}
	}
	return Command{}, false
}

// delegatedNames are the `dev map` subcommands the Rust kernel implements
// natively, verified against `devmap --help` on 2026-09-06. Porting these to Go
// would add a forward-only layer over a CLI that already answers correctly.
var delegatedNames = []string{
	"affected", "clones", "dead", "explore", "impact",
	"preview", "savings", "search", "trace", "workspace",
}

func buildRegistry() []Command {
	cmds := []Command{
		{
			Name:    "status",
			Summary: "Report the index's self-report: generation, counts, freshness.",
			Run:     runStatus,
		},
		{
			Name:    "doctor",
			Summary: "Check that a usable kernel and a committed generation exist.",
			Run:     runDoctor,
		},
	}
	for _, n := range delegatedNames {
		n := n
		cmds = append(cmds, Command{
			Name:      n,
			Summary:   "Delegated to the devmap kernel.",
			Delegated: true,
			Run: func(ctx context.Context, env *Env, args []string) (Payload, error) {
				return passthrough(ctx, env, n, args)
			},
		})
	}
	sort.Slice(cmds, func(i, j int) bool { return cmds[i].Name < cmds[j].Name })
	return cmds
}

func usage(w io.Writer) {
	if w == nil {
		return
	}
	var b strings.Builder
	b.WriteString("dcmap — DevCouncil repository map\n\nUsage:\n  dcmap [--json] [--root DIR] <command> [args]\n\nCommands:\n")
	for _, c := range registry {
		mark := ""
		if c.Delegated {
			mark = " (kernel)"
		}
		fmt.Fprintf(&b, "  %-10s %s%s\n", c.Name, c.Summary, mark)
	}
	b.WriteString("\nGlobal flags:\n")
	b.WriteString("  --json        Emit exactly one JSON object on stdout; diagnostics on stderr.\n")
	b.WriteString("  --root DIR    Repository to operate on (default: current directory).\n")
	b.WriteString("\nThe devmap kernel is DEVMAP_BINARY when set (used or refused by name, never\n")
	b.WriteString("replaced), else the newest capable build under the repository's rust/target,\n")
	b.WriteString("else PATH.\n")
	io.WriteString(w, b.String())
}

// ErrNoBinary reports that no usable devmap kernel was found. It is a sentinel
// so callers can distinguish "the map is broken" from "there is no engine".
var ErrNoBinary = errors.New("no usable devmap kernel found")
