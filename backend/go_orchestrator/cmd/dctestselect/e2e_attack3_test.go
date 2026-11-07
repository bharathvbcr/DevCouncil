//go:build e2e

package main

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// Attack round 3 and the round-4 proofs, against real cargo and real go. Every
// fixture lives in t.TempDir() with its own CARGO_TARGET_DIR inside it.

// The declared test targets of the real tree that cargo runs under --workspace
// (devmap-analyze, devmap-query, devmap-store and devmap-extract gate tests on
// `parse`, a default feature) are selected with their crate and its dependents.
// The closures are logged: they are the fast phase's fan-out.
func TestE2EDefaultFeatureTestTargetsOfTheRealTreeAreSelectedWithTheirCrateAndDependents(t *testing.T) {
	root, err := filepath.Abs("../../../..")
	if err != nil {
		t.Fatal(err)
	}
	ws, err := loadRust(context.Background(), execRunner{timeout: 3 * time.Minute}, root, "rust")
	if err != nil {
		t.Fatal(err)
	}
	for _, name := range []string{"devmap-extract", "devmap-analyze", "devmap-cli"} {
		t.Logf("dependents of %s (%d of %d members): %v", name, len(ws.Dependents[name]), len(ws.Packages), ws.Dependents[name])
	}
	const file = "rust/devmap-analyze/tests/c_family_liveness.rs"
	in := inputs(withChanged(file), withTests(file))
	in.Rust = ws
	in.RustDir = "rust"
	p := BuildPlan(in)
	t.Logf("everything=%v widened=%v selected=%v dropped=%v", p.Everything, p.Widened, cmds(p), p.Dropped)
	if p.Everything || !selectsCrate(p, "devmap-analyze") {
		t.Fatalf("%s is a declared test target of devmap-analyze gated on `parse`, a default feature; selected=%v dropped=%v", file, cmds(p), p.Dropped)
	}
	excluded := map[string]bool{}
	for _, ex := range DefaultRustExcludes {
		excluded[ex] = true
	}
	for _, dep := range ws.Dependents["devmap-analyze"] {
		if !excluded[dep] && !selectsCrate(p, dep) {
			t.Errorf("%s depends on devmap-analyze and is not selected with it: %v", dep, cmds(p))
		}
	}
	cargo := 0
	for _, s := range p.Selected {
		if s.Argv[0] == "cargo" {
			cargo++
		}
	}
	if cargo != 1 {
		t.Errorf("%d cargo commands selected; the selected crates must build in one so cargo unifies their features", cargo)
	}
}

func a3write(t *testing.T, root, rel, body string) {
	t.Helper()
	p := filepath.Join(root, filepath.FromSlash(rel))
	if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(p, []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
}

// a3fixture writes a cargo workspace under rust/ of a fresh temp dir and reads
// it back with loadRust, as the job does.
func a3fixture(t *testing.T, files map[string]string) (root string, ws *RustWorkspace) {
	t.Helper()
	d, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	for rel, body := range files {
		a3write(t, d, rel, body)
	}
	cmd := exec.Command("cargo", "generate-lockfile", "--offline")
	cmd.Dir = filepath.Join(d, "rust")
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("%v\n%s", err, out)
	}
	ws, err = loadRust(context.Background(), execRunner{timeout: 2 * time.Minute}, d, "rust")
	if err != nil {
		t.Fatal(err)
	}
	return d, ws
}

// a3cargo runs a plan's own cargo command (or the full suite's) once per
// fixture, keeping its output: a failing test exits 101, which is the point.
func a3cargo(t *testing.T, root string, logs map[string]string, argv []string) string {
	t.Helper()
	key := strings.Join(argv, " ")
	if v, ok := logs[key]; ok {
		return v
	}
	args := append(append([]string{}, argv[1:]...), "--offline")
	cmd := exec.Command(argv[0], args...)
	cmd.Dir = filepath.Join(root, "rust")
	cmd.Env = append(os.Environ(), "CARGO_TERM_COLOR=never", "CARGO_TARGET_DIR="+filepath.Join(root, "target"))
	out, _ := cmd.CombinedOutput()
	logs[key] = string(out)
	return logs[key]
}

var a3full = []string{"cargo", "test", "--workspace", "--no-fail-fast"}

// a3audit audits a plan against the logs of the full suite and of the fast
// phase, as the job does, with the base known and empty.
func a3audit(t *testing.T, p Plan, fullLog, selLog string) (Recall, int) {
	t.Helper()
	in := AuditInput{Plan: p, Rust: parseLibtest(strings.NewReader(fullLog)), Go: examined(), Base: []string{},
		SelectedRust: parseLibtest(strings.NewReader(selLog)), SelectedGo: examined(), SelectedKnown: true}
	if !in.Rust.Examined || !in.SelectedRust.Examined {
		t.Fatalf("a log was not examined: full %q selected %q", in.Rust.Note, in.SelectedRust.Note)
	}
	r, _ := Audit(in)
	return r, Exit(in, r)
}

func a3plan(ws *RustWorkspace, changed string) Plan {
	in := inputs(withChanged(changed), withTests(changed))
	in.Rust = ws
	return BuildPlan(in)
}

func selectsCrate(p Plan, pkg string) bool {
	for _, s := range p.Selected {
		for _, u := range s.Units {
			if u.Kind == "rust" && u.Pkg == pkg {
				return true
			}
		}
	}
	return false
}

func keyed(r Recall, name string) bool {
	for _, k := range r.Full.Failed {
		if strings.HasSuffix(k, ":"+name) {
			return true
		}
	}
	return false
}

// Crate a: a lib, `gated` (required-features on a default feature, so
// --workspace runs it), and `common`, which is a declared target and also a
// module of `it`.
var a3crateFiles = map[string]string{
	"rust/Cargo.toml":        "[workspace]\nresolver = \"3\"\nmembers = [\"a\", \"z\"]\n",
	"rust/a/Cargo.toml":      "[package]\nname = \"a\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[features]\ndefault = [\"fast\"]\nfast = []\n\n[[test]]\nname = \"gated\"\nrequired-features = [\"fast\"]\n",
	"rust/a/src/lib.rs":      "",
	"rust/a/tests/gated.rs":  "#[test]\nfn gated_bad() { assert!(false); }\n",
	"rust/a/tests/common.rs": "#[test]\nfn c_bad() { assert!(false); }\n",
	"rust/a/tests/it.rs":     "mod common;\n",
	// A second member: cargo names the package in its `to rerun pass` line only
	// when the workspace has more than one, as the real tree does.
	"rust/z/Cargo.toml": "[package]\nname = \"z\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
	"rust/z/src/lib.rs": "",
}

// A test file another target includes, and a target gated on a default feature,
// both run in the full suite. The selection is the crate, so the fast phase runs
// every harness that compiles the file and fails every test the full suite does.
func TestE2ECrateSelectionRunsEveryTargetTheFullSuiteRunsInTheCrate(t *testing.T) {
	root, ws := a3fixture(t, a3crateFiles)
	logs := map[string]string{}
	for _, tc := range []struct{ file, test string }{
		{"rust/a/tests/common.rs", "common::c_bad"},
		{"rust/a/tests/gated.rs", "gated_bad"},
	} {
		p := a3plan(ws, tc.file)
		if p.Everything || len(p.Selected) != 1 || p.Selected[0].Display != "cargo test --locked -p a --no-fail-fast" {
			t.Fatalf("%s: selected %v dropped=%v", tc.file, cmds(p), p.Dropped)
		}
		r, code := a3audit(t, p, a3cargo(t, root, logs, a3full), a3cargo(t, root, logs, p.Selected[0].Argv))
		if !keyed(r, tc.test) {
			t.Fatalf("fixture: the full suite did not fail %s: %v", tc.test, r.Full.Failed)
		}
		if len(r.Missed) != 0 || len(r.SelectedNotReproduced) != 0 || r.Recall == nil || *r.Recall != 1 || code != 0 {
			t.Errorf("%s: missed=%v not_reproduced=%v recall=%v exit=%d; full failed %v", tc.file, r.Missed, r.SelectedNotReproduced, r.Recall, code, r.Full.Failed)
		}
	}
}

// Crate a's lib has a test behind `extra`, which only b's dependency on a turns
// on. --workspace builds a with it. Selecting a with its dependent in one cargo
// command builds it the same way, so the fast phase fails the same test.
var a3pairFiles = map[string]string{
	"rust/Cargo.toml":   "[workspace]\nresolver = \"3\"\nmembers = [\"a\", \"b\"]\n",
	"rust/a/Cargo.toml": "[package]\nname = \"a\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[features]\nextra = []\n",
	"rust/a/src/lib.rs": "#[cfg(feature = \"extra\")]\n#[test]\nfn extra_bad() { assert!(false); }\n",
	"rust/b/Cargo.toml": "[package]\nname = \"b\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\na = { path = \"../a\", features = [\"extra\"] }\n",
	"rust/b/src/lib.rs": "",
}

func TestE2EACrateAndItsDependentBuildInOneCommandWithTheFeaturesTheFullSuiteBuildsThemWith(t *testing.T) {
	root, ws := a3fixture(t, a3pairFiles)
	logs := map[string]string{}
	p := a3plan(ws, "rust/a/src/lib.rs")
	if len(p.Selected) != 1 || p.Selected[0].Display != "cargo test --locked -p a -p b --no-fail-fast" {
		t.Fatalf("selected %v: b depends on a, so the crates build together", cmds(p))
	}
	r, code := a3audit(t, p, a3cargo(t, root, logs, a3full), a3cargo(t, root, logs, p.Selected[0].Argv))
	if !keyed(r, "extra_bad") {
		t.Fatalf("fixture: --workspace did not run extra_bad: %v", r.Full.Failed)
	}
	if r.Recall == nil || *r.Recall != 1 || len(r.SelectedNotReproduced) != 0 || code != 0 {
		t.Fatalf("recall=%v not_reproduced=%v exit=%d: the fast phase did not run extra_bad", r.Recall, r.SelectedNotReproduced, code)
	}
}

// If a phase difference remains (here, the old per-crate command `-p a`, which
// builds a without `extra`), it is priced, not hidden: recall is 0, the failure
// is listed in selected_not_reproduced, and the audit does not exit 1.
func TestE2EAPhaseDifferenceIsPricedInRecallAndListedNotCountedCaughtOrMissed(t *testing.T) {
	root, ws := a3fixture(t, a3pairFiles)
	logs := map[string]string{}
	p := a3plan(ws, "rust/a/src/lib.rs")
	p.Selected[0].Units = p.Selected[0].Units[:1]
	r, code := a3audit(t, p, a3cargo(t, root, logs, a3full), a3cargo(t, root, logs, []string{"cargo", "test", "--locked", "-p", "a", "--no-fail-fast"}))
	if r.Recall == nil || *r.Recall != 0 || len(r.SelectedNotReproduced) != 1 || len(r.Missed) != 0 || code != 0 {
		t.Fatalf("recall=%v not_reproduced=%v missed=%v exit=%d; full failed %v", r.Recall, r.SelectedNotReproduced, r.Missed, code, r.Full.Failed)
	}
}

// Cargo compiles every requested target before running any test, so a crate
// that fails to build leaves no `test result:` line at all, whatever the other
// crate's tests would have done. A non-zero exit with no `test result: FAILED`
// is therefore a command that ran nothing, and is not examined.
func TestE2ECargoLeavesNoTestResultWhenOneRequestedCrateFailsToCompile(t *testing.T) {
	root, ws := a3fixture(t, map[string]string{
		"rust/Cargo.toml":   "[workspace]\nresolver = \"3\"\nmembers = [\"x\", \"y\"]\n",
		"rust/x/Cargo.toml": "[package]\nname = \"x\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
		"rust/x/src/lib.rs": "pub fn broken( {\n",
		"rust/y/Cargo.toml": "[package]\nname = \"y\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
		"rust/y/src/lib.rs": "#[test]\nfn y_bad() { assert!(false); }\n",
	})
	in := inputs(withChanged("rust/x/src/lib.rs", "rust/y/src/lib.rs"))
	in.Rust = ws
	p := BuildPlan(in)
	if len(p.Selected) != 1 || p.Selected[0].Display != "cargo test --locked -p x -p y --no-fail-fast" {
		t.Fatalf("selected %v", cmds(p))
	}
	out := a3cargo(t, root, map[string]string{}, p.Selected[0].Argv)
	t.Logf("cargo output:\n%s", out)
	for _, l := range strings.Split(out, "\n") {
		if strings.HasPrefix(l, "test result:") {
			t.Fatalf("cargo printed %q although x failed to compile: the rule that a non-zero exit with no FAILED result ran nothing is wrong at this cargo", l)
		}
	}
	log := parseLibtest(strings.NewReader(out))
	if log.Examined || log.FailedResults != 0 {
		t.Errorf("log = %+v; a build failure is not an examined suite", log)
	}
	if why := unexaminedUnits(p.Selected[0], &ExitInfo{Code: 101, Completed: true}, SuiteRun{Examined: true}); why == "" {
		t.Error("a non-zero exit with no `test result: FAILED` was examined")
	}
}

// go test -json for a package that fails to build: the go in go.mod prints
// build-output and build-fail actions and a final fail for the package; older go
// printed raw text. The examination rule reads a final action per selected
// package, so it does not depend on which shape appears.
func TestE2EAGoPackageThatFailsToBuildIsExaminedAsFailedOrNotExaminedNeverAsRun(t *testing.T) {
	dir := t.TempDir()
	a3write(t, dir, "go.mod", "module example.com/gb\n\ngo 1.26\n")
	a3write(t, dir, "ok/ok.go", "package ok\n")
	a3write(t, dir, "ok/ok_test.go", "package ok\n\nimport \"testing\"\n\nfunc TestOK(t *testing.T) {}\n")
	a3write(t, dir, "bad/bad.go", "package bad\n\nfunc F( {\n")
	a3write(t, dir, "bad/bad_test.go", "package bad\n\nimport \"testing\"\n\nfunc TestB(t *testing.T) {}\n")
	if v, err := exec.Command("go", "version").Output(); err == nil {
		t.Logf("%s", v)
	}
	cmd := exec.Command("go", "test", "-count=1", "-json", "example.com/gb/ok", "example.com/gb/bad")
	cmd.Dir = dir
	cmd.Env = append(os.Environ(), "GOFLAGS=", "GOTOOLCHAIN=local")
	out, _ := cmd.CombinedOutput()
	t.Logf("go test -json output:\n%s", out)
	run := parseGoJSON(strings.NewReader(string(out)))
	if !run.Done["example.com/gb/ok"] {
		t.Fatalf("the passing package has no final action: %+v", run)
	}
	if run.Done["example.com/gb/bad"] {
		// The json shape: the package failed, and is a failure in the log.
		if len(run.Failures) != 1 || run.Failures[0].Pkg != "example.com/gb/bad" || run.Failures[0].Name != "[package]" {
			t.Errorf("failures = %+v", run.Failures)
		}
	} else if run.Examined {
		t.Errorf("a package with no final action left an examined log: %+v", run)
	}
}
