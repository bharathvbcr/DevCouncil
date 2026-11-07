//go:build e2e

package main

import (
	"context"
	"path/filepath"
	"testing"
	"time"
)

func mustRoot(t *testing.T) string {
	t.Helper()
	root, err := filepath.Abs("../../../..")
	if err != nil {
		t.Fatal(err)
	}
	return root
}

// Real tree (cargo metadata only, nothing built): a change in dc-proc or
// dc-redact, which the full suite excludes, selects the non-excluded crates
// that depend on it, and not the excluded crate.
func TestE2ERealTreeChangeInAnExcludedCrateSelectsItsNonExcludedDependents(t *testing.T) {
	ws, err := loadRust(context.Background(), execRunner{timeout: 3 * time.Minute}, mustRoot(t), "rust")
	if err != nil {
		t.Fatal(err)
	}
	for _, crate := range []string{"dc-proc", "dc-redact"} {
		file := "rust/" + crate + "/src/lib.rs"
		in := inputs(withChanged(file), withTests(file))
		in.Rust, in.RustDir = ws, "rust"
		p := BuildPlan(in)
		t.Logf("%s: dependents=%v everything=%v selected=%v dropped=%v", crate, ws.Dependents[crate], p.Everything, cmds(p), p.Dropped)
		if p.Everything {
			t.Errorf("%s changed: widened to everything: %v", file, p.Widened)
			continue
		}
		if !selectsCrate(p, "devmap-store") {
			t.Errorf("%s changed; devmap-store depends on it and is in the full suite; selected=%v", file, cmds(p))
		}
		if selectsCrate(p, crate) {
			t.Errorf("%s is excluded from the full suite and must not be run: %v", crate, cmds(p))
		}
	}
}

// Fixture: crate dc-proc (excluded by DefaultRustExcludes) and its dependent
// user, whose test fails because dc-proc changed. The full suite fails it, so
// the fast phase must select user, and run it against the changed dc-proc.
func TestE2EChangeInAnExcludedCrateSelectsTheDependentTheFullSuiteFails(t *testing.T) {
	root, ws := a3fixture(t, map[string]string{
		"rust/Cargo.toml":         "[workspace]\nresolver = \"3\"\nmembers = [\"dc-proc\", \"user\"]\n",
		"rust/dc-proc/Cargo.toml": "[package]\nname = \"dc-proc\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
		"rust/dc-proc/src/lib.rs": "pub fn v() -> u32 { 2 }\n",
		"rust/user/Cargo.toml":    "[package]\nname = \"user\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ndc-proc = { path = \"../dc-proc\" }\n",
		"rust/user/src/lib.rs":    "#[test]\nfn user_bad() { assert_eq!(dc_proc::v(), 1); }\n",
	})
	logs := map[string]string{}
	p := a3plan(ws, "rust/dc-proc/src/lib.rs")
	full := a3cargo(t, root, logs, []string{"cargo", "test", "--workspace", "--exclude", "dc-proc", "--no-fail-fast"})
	if p.Everything || len(p.Selected) != 1 || p.Selected[0].Display != "cargo test --locked -p user --no-fail-fast" {
		t.Fatalf("selected %v everything=%v; user_bad fails in the full suite:\n%s", cmds(p), p.Everything, full)
	}
	r, code := a3audit(t, p, full, a3cargo(t, root, logs, p.Selected[0].Argv))
	if !keyed(r, "user_bad") || !a3selectedFailed(a3cargo(t, root, logs, p.Selected[0].Argv), "user_bad") || len(r.SelectedPhaseFailedOnly) != 0 || code != 0 {
		t.Fatalf("selected_only=%v exit=%d: the fast phase must fail user_bad as the full suite does; full failed %v", r.SelectedPhaseFailedOnly, code, r.Full.Failed)
	}
}
