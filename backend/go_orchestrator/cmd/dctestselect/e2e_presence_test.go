//go:build e2e

package main

import (
	"context"
	"strings"
	"testing"
	"time"
)

// A crate with no test target cargo runs prints no section, and a selected
// command over it and a crate that does run must still be examined. The header
// of the crate that runs names its source relative to the package and a binary
// named for the crate, which is what the audit looks for.
func TestE2EPresenceACrateWithNothingToRunIsNotHeldAgainstTheRun(t *testing.T) {
	root, ws := a3fixture(t, map[string]string{
		"rust/Cargo.toml":          "[workspace]\nresolver = \"3\"\nmembers = [\"a\", \"n\"]\n",
		"rust/a/Cargo.toml":        "[package]\nname = \"a\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
		"rust/a/src/lib.rs":        "#[test]\nfn a_ok() {}\n",
		"rust/a/src/bin/a-tool.rs": "fn main() {}\n#[test]\nfn tool_ok() {}\n",
		"rust/n/Cargo.toml":        "[package]\nname = \"n\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\ntest = false\ndoctest = false\n",
		"rust/n/src/lib.rs":        "pub fn f() {}\n",
	})
	in := inputs(withChanged("rust/a/src/lib.rs", "rust/n/src/lib.rs"))
	in.Rust, in.RustDir = ws, "rust"
	p := BuildPlan(in)
	if len(p.Selected) != 1 {
		t.Fatalf("selected %v", cmds(p))
	}
	sel := p.Selected[0]
	runs := map[string][]UnitRun{}
	for _, u := range sel.Units {
		runs[u.Pkg] = u.Runs
	}
	if len(runs["a"]) != 2 || len(runs["n"]) != 0 {
		t.Fatalf("runs = %v; a has a lib and a hyphenated bin, n has none", runs)
	}
	out := a3cargo(t, root, map[string]string{}, sel.Argv)
	t.Logf("cargo output:\n%s", out)
	if !strings.Contains(out, "Running unittests src/lib.rs (") || !strings.Contains(out, "/deps/a-") {
		t.Fatalf("cargo's header is not the package-relative source and crate-named binary the audit looks for:\n%s", out)
	}
	log := parseLibtest(strings.NewReader(out))
	if !log.Examined {
		t.Fatalf("log not examined: %s", log.Note)
	}
	if why := unexaminedUnits(sel, &ExitInfo{Code: 0, Completed: true}, log); why != "" {
		t.Errorf("the crate with nothing to run was held against the run: %s", why)
	}
	// Every test target of a left its own section, each found by its own header
	// (the hyphenated bin's binary is named with an underscore).
	for _, r := range runs["a"] {
		if !log.Sources[r] {
			t.Errorf("no section for %+v; sources = %v", r, log.Sources)
		}
	}
	// Drop a's section from the same log: now a, which has a test target, ran nothing.
	if why := unexaminedUnits(sel, &ExitInfo{Code: 0, Completed: true}, SuiteRun{Examined: true}); !strings.Contains(why, "ran nothing for a") {
		t.Errorf("a crate with a test target and no section was not caught: %q", why)
	}
}

// On the real tree (cargo metadata only, nothing built): which members the
// presence check leaves out. A member is left out only when no test target of
// it runs without a feature switched on.
func TestE2EPresenceRealTreeMembersLeftOutOfTheCheck(t *testing.T) {
	root := mustRoot(t)
	ws, err := loadRust(context.Background(), execRunner{timeout: 3 * time.Minute}, root, "rust")
	if err != nil {
		t.Fatal(err)
	}
	var left []string
	for _, pkg := range ws.Packages {
		if len(rustUnit(pkg).Runs) == 0 {
			left = append(left, pkg.Name)
		}
	}
	t.Logf("%d members; left out of the presence check: %v", len(ws.Packages), left)
	if len(left) != 0 {
		t.Errorf("members with no test target that runs by default: %v; the presence check would not cover them", left)
	}
}
