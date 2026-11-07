package main

import (
	"strings"
	"testing"
)

// A changed .rs file under src/ of a main-workspace member that the full suite
// excludes (dc-proc, dc-redact) is not run itself, but `cargo test --workspace
// --exclude ...` still builds its non-excluded dependents against the changed
// crate and runs their tests, so the selection must include them.
func TestAChangeInAnExcludedCrateSelectsItsNonExcludedDependentsAndNotTheCrate(t *testing.T) {
	in := inputs(withChanged("rust/dc-proc/src/lib.rs"))
	in.Rust.Packages = append(in.Rust.Packages,
		RustPackage{Name: "dc-proc", Dir: "rust/dc-proc"},
		RustPackage{Name: "devmap-store", Dir: "rust/devmap-store"})
	in.Rust.Dependents = map[string][]string{"dc-proc": {"devmap-store"}}
	p := BuildPlan(in)
	if p.Everything {
		t.Fatalf("a change in an excluded crate with a dependent widened to everything: %v", p.Widened)
	}
	var cargo []string
	for _, c := range cmds(p) {
		if strings.HasPrefix(c, "cargo ") {
			cargo = append(cargo, c)
		}
	}
	if len(cargo) != 1 || cargo[0] != "cargo test --locked -p devmap-store --no-fail-fast" {
		t.Fatalf("cargo commands %v of %v; want one over devmap-store only (dc-proc is excluded from the full suite)", cargo, cmds(p))
	}
	var logged bool
	for _, d := range p.Dropped {
		logged = logged || d.Path == "rust/dc-proc/src/lib.rs"
	}
	if !logged {
		t.Errorf("the excluded crate's own change is not logged as not run: dropped=%v", p.Dropped)
	}
}

// With no non-excluded dependent there is nothing to run: no cargo command at
// all, since one with no -p would run the whole workspace.
func TestAChangeInAnExcludedCrateWithNoDependentsSelectsNoCargoCommand(t *testing.T) {
	in := inputs(withChanged("rust/dc-proc/src/lib.rs"))
	in.Rust.Packages = append(in.Rust.Packages, RustPackage{Name: "dc-proc", Dir: "rust/dc-proc"})
	in.Rust.Dependents = map[string][]string{}
	for _, c := range cmds(BuildPlan(in)) {
		if strings.HasPrefix(c, "cargo ") {
			t.Fatalf("selected %q for a change that reaches no test the full suite runs", c)
		}
	}
}
