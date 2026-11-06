package main

import (
	"encoding/json"
	"fmt"
	"path"
	"regexp"
	"sort"
	"strings"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/devmap"
)

const planSchema = 1

// Widening records why the selection is wider than the blast's own test list.
// Each one is logged as it is made: a selection that grew for a reason nobody
// can read back is indistinguishable from one that grew by accident.
type Widening struct {
	Scope  string `json:"scope"`
	Reason string `json:"reason"`
}

// Unit is what a command covers, in the terms a failure is reported in.
//
// Audit decides whether a failed test was inside the selection by matching its
// package and target against these, so the unit is the same shape cargo prints
// in its `to rerun pass` line, not a path this program invented.
type Unit struct {
	Kind string `json:"kind"` // "rust" or "go"
	// Pkg is a cargo package name or a Go import path.
	Pkg string `json:"pkg"`
	// Target is `--lib`, `--bin x` or `--test x`. Empty means every target of
	// the package, doctests included.
	Target string `json:"target,omitempty"`
	// Filters are libtest substring filters. Empty means the whole target.
	Filters []string `json:"filters,omitempty"`
}

// Selected is one command of the fast phase.
type Selected struct {
	Command
	Units []Unit `json:"units"`
	// Files are the blast's test paths this command was built from. Empty for
	// a widened command, whose Why says what it was built from instead.
	Files []string `json:"files"`
	// Seeds are the blast seeds that sit in this command's own package. A test
	// reached through the graph from a seed in another package has none here;
	// the blast gives no per-test attribution, so none is invented.
	Seeds []string `json:"seeds"`
	Why   string   `json:"why"`
}

// Dropped is a blast test path that no command of this job's full suite runs.
type Dropped struct {
	Path   string `json:"path"`
	Reason string `json:"reason"`
}

// BlastSummary is the part of the blast worth keeping in an artifact.
type BlastSummary struct {
	Change      string            `json:"change"`
	Complete    bool              `json:"complete"`
	Unavailable []json.RawMessage `json:"unavailable"`
	SeedsTotal  int               `json:"seeds_total"`
	Seeds       []string          `json:"seeds"`
	TestsTotal  int               `json:"tests_total"`
}

// Plan is the selector's whole decision.
type Plan struct {
	Schema  int           `json:"schema"`
	Base    string        `json:"base"`
	Head    string        `json:"head"`
	Trigger string        `json:"trigger"`
	Blast   *BlastSummary `json:"blast"`
	// Everything means the selection is the full suite, so the fast phase is
	// skipped and nothing can be outside the selection.
	Everything bool       `json:"everything"`
	Widened    []Widening `json:"widened"`
	Selected   []Selected `json:"selected"`
	Dropped    []Dropped  `json:"dropped"`
	Full       []Command  `json:"full"`
	Notices    []string   `json:"notices"`
}

// RustTarget is one cargo target, from `cargo metadata`.
type RustTarget struct {
	Name             string
	Kinds            []string
	Src              string // repository-relative
	RequiredFeatures []string
	Test             bool
}

func (t RustTarget) is(kind string) bool {
	for _, k := range t.Kinds {
		if k == kind {
			return true
		}
	}
	return false
}

// isLib reports a library target in any crate type.
func (t RustTarget) isLib() bool {
	return t.is("lib") || t.is("rlib") || t.is("proc-macro") || t.is("dylib") || t.is("cdylib") || t.is("staticlib")
}

// RustPackage is one workspace member.
type RustPackage struct {
	Name    string
	Dir     string // repository-relative
	Targets []RustTarget
}

// RustWorkspace is the authority for which targets exist.
type RustWorkspace struct {
	Packages []RustPackage
}

// GoPackage is one `go list` package.
type GoPackage struct {
	ImportPath string
	Dir        string   // repository-relative
	GoFiles    []string // repository-relative, non-test
	TestFiles  []string // repository-relative, in-package and external
	Imports    []string // every import of the package and of its tests
}

// GoWorkspace is the Go module's package list.
type GoWorkspace struct {
	Module   string
	Packages []GoPackage
}

// Inputs is everything BuildPlan decides from. Every field that comes from a
// process arrives with the error that process returned, so BuildPlan, not its
// caller, decides that a failed question widens the selection.
type Inputs struct {
	Base, Head, Trigger string
	BaseErr             error

	Blast    *devmap.Blast
	BlastErr error

	GitChanged []string
	GitErr     error

	Rust    *RustWorkspace
	RustErr error

	Go    *GoWorkspace
	GoErr error

	// ReadFile reads a repository-relative file. It is how a `mod x;`
	// declaration in a test harness is confirmed.
	ReadFile func(rel string) ([]byte, error)

	// Excludes are the cargo packages the job's full suite leaves out.
	Excludes []string
	RustDir  string // repository-relative, where cargo runs
	GoDir    string // repository-relative, where go runs
}

// DefaultRustExcludes is rust.yml's `--exclude` list for its `Test` step: the
// analysis-plane crates that analysis-plane.yml tests. The universe of this
// job's full suite is the workspace less these, exactly as rust.yml's.
var DefaultRustExcludes = []string{"dc-glob", "dc-proc", "dc-redact", "dc-grep", "dc-store", "dc-verify", "dc-evidence"}

const (
	maxSeedsKept   = 100
	maxSeedsPerCmd = 20
)

// everythingTrigger names the paths whose change can alter what any test
// means without touching a symbol the graph indexes.
func everythingTrigger(p, rustDir string) (string, bool) {
	switch path.Base(p) {
	case "Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "rust-toolchain", "build.rs":
		return "build definition " + p, true
	case "go.mod", "go.sum", "go.work", "go.work.sum":
		return "Go module definition " + p, true
	}
	if strings.HasPrefix(p, ".github/") {
		return "workflow or action " + p, true
	}
	for _, dir := range []string{"testsupport", "testdata", "vendor"} {
		if strings.HasPrefix(p, rustDir+"/"+dir+"/") {
			return rustDir + "/" + dir + "/ fixture or harness " + p, true
		}
	}
	return "", false
}

// BuildPlan decides what the fast phase runs.
//
// The selection is an ordering and a measurement, never a gate: the full suite
// runs after it either way. That is why every doubt widens it, and why a
// widening costs only the fast signal.
func BuildPlan(in Inputs) Plan {
	p := Plan{
		Schema: planSchema, Base: in.Base, Head: in.Head, Trigger: in.Trigger,
		Widened: []Widening{}, Selected: []Selected{}, Dropped: []Dropped{}, Notices: []string{},
		Full: fullCommands(in),
	}
	everything := func(reason string) Plan {
		p.Everything = true
		p.Selected = []Selected{}
		p.Widened = append(p.Widened, Widening{Scope: "everything", Reason: reason})
		return p
	}

	switch {
	case in.BaseErr != nil:
		return everything("the change base could not be resolved: " + in.BaseErr.Error())
	case in.BlastErr != nil:
		return everything("devmap blast failed: " + in.BlastErr.Error())
	case in.Blast == nil:
		return everything("devmap blast returned no answer")
	case in.GitErr != nil:
		return everything("git diff failed: " + in.GitErr.Error())
	case in.RustErr != nil:
		return everything("cargo metadata failed: " + in.RustErr.Error())
	case in.Rust == nil:
		return everything("cargo metadata returned no answer")
	case in.GoErr != nil:
		return everything("go list failed: " + in.GoErr.Error())
	case in.Go == nil:
		return everything("go list returned no answer")
	}
	blast := in.Blast
	p.Blast = summarize(blast)

	// The graph answers about the tree it was built from. A blast from another
	// head describes a change that is not the one under test.
	if head := blast.Head(); head == "" || head != in.Head {
		return everything(fmt.Sprintf("the devmap index head %q is not the checked-out head %q", head, in.Head))
	}

	for _, file := range changedPaths(in) {
		if reason, ok := everythingTrigger(file, in.RustDir); ok {
			return everything(reason)
		}
	}

	for _, u := range blast.Unavailable {
		switch u.Kind {
		case "impact_incomplete", "affected_tests_incomplete":
			// The selection is a lower bound, and it is meant to be: widening
			// here selects everything on every real change, because the walk
			// stops at its depth cap on all of them. The full suite gates and
			// the audit prices the cap.
			p.Notices = append(p.Notices, fmt.Sprintf("blast %s (%s): the selection is a lower bound; the full suite gates", u.Kind, describeLimit(u)))
		case "owners_unavailable":
			p.Notices = append(p.Notices, "blast owners_unavailable: ownership is not used to select")
		case "change_unattributed":
			// Resolved below, per range.
		default:
			return everything("devmap blast reports " + string(u.Raw))
		}
	}
	if hasKind(blast.Unavailable, "change_unattributed") != (len(blast.Unattributed) > 0) {
		return everything("devmap blast's change_unattributed and unattributed list disagree")
	}
	sel := newSelection(in)
	for _, u := range blast.Unattributed {
		if u.Reason == "file_not_indexed" {
			return everything("a changed file is not indexed: " + u.Path)
		}
		scope, ok := sel.widenOwner(u.Path, u.Reason)
		if !ok {
			return everything("a change outside any symbol has no owning test unit: " + u.Path)
		}
		if scope != "" {
			p.Widened = append(p.Widened, Widening{Scope: scope, Reason: "change_unattributed: " + u.Path + " (" + u.Reason + ")"})
		}
	}

	if scope, reason, ok := crossProcess(in, changedPaths(in)); ok {
		sel.widenGoImporters(in.Go)
		p.Widened = append(p.Widened, Widening{Scope: scope, Reason: reason})
	}

	for _, t := range blast.Tests {
		sel.add(t.Path)
	}
	p.Dropped = sel.dropped
	p.Selected = sel.commands(blast.Seeds)
	return p
}

func hasKind(us []devmap.BlastUnavailable, kind string) bool {
	for _, u := range us {
		if u.Kind == kind {
			return true
		}
	}
	return false
}

func describeLimit(u devmap.BlastUnavailable) string {
	var parts []string
	if u.Depth != nil {
		parts = append(parts, fmt.Sprintf("depth=%d", *u.Depth))
	}
	if u.Reached != nil {
		parts = append(parts, fmt.Sprintf("reached=%d", *u.Reached))
	}
	if len(parts) == 0 {
		return string(u.Raw)
	}
	return strings.Join(parts, " ")
}

func summarize(b *devmap.Blast) *BlastSummary {
	s := &BlastSummary{Change: b.Change, Complete: b.Complete, SeedsTotal: len(b.Seeds), TestsTotal: len(b.Tests), Unavailable: []json.RawMessage{}, Seeds: []string{}}
	for _, u := range b.Unavailable {
		s.Unavailable = append(s.Unavailable, u.Raw)
	}
	for i, seed := range b.Seeds {
		if i == maxSeedsKept {
			break
		}
		s.Seeds = append(s.Seeds, seed.QualifiedName)
	}
	return s
}

// changedPaths is every path either git or the blast calls changed. The union
// is deliberate: a path only one of them reports is exactly the case in which
// one of them is wrong.
func changedPaths(in Inputs) []string {
	seen := map[string]bool{}
	var out []string
	add := func(p string) {
		if p != "" && !seen[p] {
			seen[p] = true
			out = append(out, p)
		}
	}
	for _, p := range in.GitChanged {
		add(p)
	}
	for _, f := range in.Blast.ChangedFiles {
		add(f.Path)
		add(f.RenamedFrom)
	}
	sort.Strings(out)
	return out
}

// crossProcess implements the rule the graph cannot: no edge joins a Rust
// dc-*/devmap-cli source file to the Go tests that run its binary through
// testsupport (gap blast-no-cross-process-rust-to-go-tests-2026-11-06).
func crossProcess(in Inputs, changed []string) (scope, reason string, ok bool) {
	for _, pkg := range in.Rust.Packages {
		if !strings.HasPrefix(pkg.Name, "dc-") && pkg.Name != "devmap-cli" {
			continue
		}
		for _, file := range changed {
			if strings.HasPrefix(file, pkg.Dir+"/") {
				return "go packages importing testsupport or dc/devmap",
					"cross-process: " + file + " is in " + pkg.Name + ", whose binary Go tests run (gap blast-no-cross-process-rust-to-go-tests-2026-11-06)", true
			}
		}
	}
	return "", "", false
}

// fullCommands is the full suite's cargo command, in one place.
func fullCommands(in Inputs) []Command {
	rust := []string{"cargo", "test", "--locked", "--workspace"}
	for _, ex := range in.Excludes {
		rust = append(rust, "--exclude", ex)
	}
	rust = append(rust, "--no-fail-fast")
	cargo := newCommand(in.RustDir, rust...)
	cargo.Log = "full-rust.log"
	golang := newCommand(in.GoDir, "go", "test", "./...", "-count=1", "-json")
	golang.GoJSON = true
	golang.Log = "full-go.jsonl"
	return []Command{cargo, golang}
}

var modDecl = func(name string) *regexp.Regexp {
	return regexp.MustCompile(`(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+` + regexp.QuoteMeta(name) + `\s*[;{]`)
}
