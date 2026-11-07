package main

import (
	"bytes"
	"context"
	"path/filepath"
	"strings"
	"testing"
)

// One invariant: an unexamined suite never produces `missed`, exit 0, or a
// recall number. Each test here is a probe of the round-3 review turned into a
// check of the behaviour that invariant requires.

// knownBase is a base artifact that failed nothing: the base is known, so a
// failure outside the selection would be a miss if the suites were examined.
func knownBase(t *testing.T, dir string) string {
	t.Helper()
	return writeFile(t, dir, "base.json", string(baseDoc(func(s string) string { return strings.Replace(s, `["a","b"]`, `[]`, 1) })))
}

func requireNoVerdict(t *testing.T, code int, stdout string, r Recall) {
	t.Helper()
	if code != 2 {
		t.Errorf("exit = %d, want 2 (not examined)\n%s", code, stdout)
	}
	if len(r.Missed) != 0 {
		t.Errorf("missed = %v; an unexamined suite names no miss", r.Missed)
	}
	if strings.Contains(stdout, "selection missed") {
		t.Errorf("a `selection missed` line for an unexamined suite:\n%s", stdout)
	}
	if r.Recall != nil {
		t.Errorf("recall = %v, want null", *r.Recall)
	}
}

func TestAFullSuiteWithNoStatusNamesNoMissEvenWithAKnownBase(t *testing.T) {
	dir := phaseDir(t, selectedCa, plantedMiss, goPass, nil)
	code, stdout, r := auditIn(t, dir, "--base-artifact", knownBase(t, dir))
	requireNoVerdict(t, code, stdout, r)
	if r.Full.Rust.Examined {
		t.Error("a suite with no exit status was examined")
	}
	if len(r.Unclassified) != 1 {
		t.Errorf("unclassified = %v, want the one failure", r.Unclassified)
	}
}

func TestACutOffBinaryNamesNoMissEvenWithAKnownBase(t *testing.T) {
	cut := "     Running tests/it.rs (t/it-1)\n\nrunning 2 tests\ntest m::boom ... FAILED\n"
	dir := phaseDir(t, selectedCa, cut, goPass, finished(101))
	code, stdout, r := auditIn(t, dir, "--base-artifact", knownBase(t, dir))
	requireNoVerdict(t, code, stdout, r)
	if r.Full.Rust.Examined {
		t.Error("a cut-off binary was examined")
	}
}

func TestAFailureWithNoKnownPackageIsNeverAMiss(t *testing.T) {
	phantom := Failure{Kind: "rust", Pkg: "?", Name: "m::boom"}
	r, notes := Audit(AuditInput{Plan: selectedCa, Rust: examined(phantom), Go: examined(), Base: []string{}})
	if len(r.Missed) != 0 || len(r.Unclassified) != 1 {
		t.Errorf("missed=%v unclassified=%v; a failure cargo never attributed cannot be a miss", r.Missed, r.Unclassified)
	}
	for _, n := range notes {
		if strings.Contains(n, "selection missed") {
			t.Errorf("note %q", n)
		}
	}
}

func TestAFailureWithNoKnownPackageExitsTwoThroughTheCommand(t *testing.T) {
	log := "     Running tests/x.rs (t/x)\n\nrunning 1 test\ntest boom ... FAILED\n\ntest result: FAILED. 0 passed; 1 failed\n"
	dir := phaseDir(t, selectedCa, log, goPass, finished(101))
	code, stdout, r := auditIn(t, dir, "--base-artifact", knownBase(t, dir))
	if code != 2 || len(r.Missed) != 0 || strings.Contains(stdout, "selection missed") {
		t.Errorf("exit=%d missed=%v\n%s", code, r.Missed, stdout)
	}
}

// A selected command that completed (cargo exits 101) but whose log shows no
// test ran covers nothing: the full suite's failure in that crate is not
// "inside the selection".
func TestASelectedCommandThatCompiledNothingIsNotACoveredSelection(t *testing.T) {
	failCd := "     Running unittests src/lib.rs (t/l)\n\nrunning 1 test\ntest engine::t::bad ... FAILED\n\nfailures:\n\n---- engine::t::bad stdout ----\n\nfailures:\n    engine::t::bad\n\ntest result: FAILED. 0 passed; 1 failed\nerror: test failed, to rerun pass `-p cd --lib`\n"
	plan := selectedCa
	plan.Selected = []Selected{
		{Command: logged(newCommand("rust", "cargo", "test", "-p", "cd")), Units: []Unit{{Kind: "rust", Pkg: "cd"}}},
	}
	dir := phaseDir(t, plan, failCd, goPass, finished(101))
	writeFile(t, dir, "selected-rust.log", "   Compiling cd v0.1.0\nerror: could not compile `cd` (lib test) due to 1 previous error\n")
	code, stdout, r := auditIn(t, dir, "--base-artifact", knownBase(t, dir))
	requireNoVerdict(t, code, stdout, r)
	if !strings.Contains(r.RecallNote, "fast phase was not examined") {
		t.Errorf("recall_note = %q", r.RecallNote)
	}
}

// A plan that lists one suite must not make the full phase green without the
// other: the full phase runs the built-in suite whatever the plan says.
func TestAOneSuitePlanNeverMakesTheFullPhaseGreenWithoutTheRustSuite(t *testing.T) {
	dir := t.TempDir() // no rust/ directory: the built-in cargo command cannot start
	ok := helper("ok")
	ok.Log = "full-go.jsonl"
	planPath := filepath.Join(dir, "plan.json")
	if err := writeAtomic(planPath, Plan{Schema: planSchema, Selected: []Selected{}, Full: []Command{ok}}); err != nil {
		t.Fatal(err)
	}
	var out bytes.Buffer
	code := run(context.Background(), []string{"run", "--plan", planPath, "--phase", "full", "--repo", dir, "--log-dir", dir}, &out, &bytes.Buffer{})
	if code == 0 {
		t.Fatalf("a plan with only the Go suite made the full phase green\n%s", out.String())
	}
	if !strings.Contains(out.String(), "cargo test --locked --workspace") {
		t.Errorf("the built-in Rust suite was never attempted:\n%s", out.String())
	}
	if !strings.Contains(out.String(), "::warning::") {
		t.Errorf("the plan's own list was ignored silently:\n%s", out.String())
	}
}

// useFullSuite makes the full phase run cmds, for tests that drive the real
// `run` command with a stand-in for cargo and go.
func useFullSuite(t *testing.T, cmds ...Command) {
	t.Helper()
	prev := fullSuite
	fullSuite = func() []Command { return cmds }
	t.Cleanup(func() { fullSuite = prev })
}
