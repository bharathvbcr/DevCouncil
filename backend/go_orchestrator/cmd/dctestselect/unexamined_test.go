package main

import (
	"bytes"
	"context"
	"path/filepath"
	"strings"
	"testing"
)

// One invariant: a suite that could not be examined is recorded as not
// examined and exits 2, never 0. Each test here is a probe of the round-3
// review turned into a check of the behaviour that invariant requires.

func requireNotExamined(t *testing.T, code int, stdout string) {
	t.Helper()
	if code != 2 {
		t.Errorf("exit = %d, want 2 (not examined)\n%s", code, stdout)
	}
	if strings.Contains(stdout, "selection missed") {
		t.Errorf("a verdict about the selection:\n%s", stdout)
	}
}

func TestAFullSuiteWithNoStatusIsNotExamined(t *testing.T) {
	dir := phaseDir(t, selectedCa, plantedMiss, goPass, nil)
	code, stdout, r := auditIn(t, dir)
	requireNotExamined(t, code, stdout)
	if r.Full.Rust.Examined {
		t.Error("a suite with no exit status was examined")
	}
}

func TestACutOffBinaryIsNotExamined(t *testing.T) {
	cut := "     Running tests/it.rs (t/it-1)\n\nrunning 2 tests\ntest m::boom ... FAILED\n"
	dir := phaseDir(t, selectedCa, cut, goPass, finished(101))
	code, stdout, r := auditIn(t, dir)
	requireNotExamined(t, code, stdout)
	if r.Full.Rust.Examined {
		t.Error("a cut-off binary was examined")
	}
}

// A selected command that completed (cargo exits 101) but whose log shows no
// test ran is not an examined fast phase.
func TestASelectedCommandThatCompiledNothingIsNotExamined(t *testing.T) {
	failCd := "     Running unittests src/lib.rs (t/l)\n\nrunning 1 test\ntest engine::t::bad ... FAILED\n\nfailures:\n\n---- engine::t::bad stdout ----\n\nfailures:\n    engine::t::bad\n\ntest result: FAILED. 0 passed; 1 failed\nerror: test failed, to rerun pass `-p cd --lib`\n"
	plan := selectedCa
	plan.Selected = []Selected{
		{Command: logged(newCommand("rust", "cargo", "test", "-p", "cd")), Units: []Unit{{Kind: "rust", Pkg: "cd"}}},
	}
	dir := phaseDir(t, plan, failCd, goPass, finished(101))
	writeFile(t, dir, "selected-rust.log", "   Compiling cd v0.1.0\nerror: could not compile `cd` (lib test) due to 1 previous error\n")
	code, stdout, _ := auditIn(t, dir)
	requireNotExamined(t, code, stdout)
	if !strings.Contains(stdout, "the selected phase was not examined") {
		t.Errorf("stdout = %q", stdout)
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
