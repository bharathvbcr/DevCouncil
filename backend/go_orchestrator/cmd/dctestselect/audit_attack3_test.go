package main

import (
	"path/filepath"
	"strings"
	"testing"
)

// Attack round 3, reshaped by the round-4 ruling: the fast phase is one cargo
// command and one go command, each over several units.

func attack3Base(t *testing.T, dir string) string {
	t.Helper()
	return writeFile(t, dir, "base.json", string(baseDoc(func(s string) string { return strings.Replace(s, `["a","b"]`, `[]`, 1) })))
}

// selectedStatus overwrites the fast phase's status with one entry per selected
// command, in the plan's order.
func selectedStatus(t *testing.T, dir string, plan Plan, infos ...ExitInfo) {
	t.Helper()
	st := phaseStatus{Schema: statusSchema, PlanSHA256: planSHA(t, filepath.Join(dir, "plan.json"))}
	for i, s := range plan.Selected {
		infos[i].Cmd, infos[i].Log, infos[i].Completed = s.Display, s.Log, true
		st.Commands = append(st.Commands, infos[i])
	}
	if err := writeAtomic(filepath.Join(dir, "selected-status.json"), st); err != nil {
		t.Fatal(err)
	}
}

const (
	cargoCdFailed  = "     Running unittests src/lib.rs (t/l)\n\nrunning 1 test\ntest engine::t::bad ... FAILED\n\nfailures:\n\n---- engine::t::bad stdout ----\n\nfailures:\n    engine::t::bad\n\ntest result: FAILED. 0 passed; 1 failed\nerror: test failed, to rerun pass `-p cd --lib`\n"
	cargoCaPassed  = "     Running tests/foo.rs (t/foo)\n\nrunning 1 test\ntest ok ... ok\n\ntest result: ok. 1 passed; 0 failed\n"
	cargoBuildFail = "error: failed to run custom build command for `cd v0.1.0`\n"
)

// A selected cargo command that exited 101 having run no test (a failed build
// script prints "error: failed to run custom build command", not "could not
// compile"; cargo compiles every requested target before running any) ran
// nothing for any of its crates. Another crate's `test result:` cannot be in the
// log, and a fabricated one must not make the command look run: the exit status
// is read against the log, and a non-zero exit with no `test result: FAILED`
// is not examined.
func TestASelectedCargoCommandThatExited101WithNoFailedResultRanNothingAndYieldsNoVerdict(t *testing.T) {
	for name, log := range map[string]string{
		"only the build error":              cargoBuildFail,
		"a passing result beside the error": cargoCaPassed + cargoBuildFail,
	} {
		dir := phaseDir(t, selectedCa, cargoCdFailed, goPass, finished(101))
		selectedStatus(t, dir, selectedCa, ExitInfo{Code: 101, Detail: "exit status 101"})
		writeFile(t, dir, "selected-rust.log", log)
		code, stdout, r := auditIn(t, dir, "--base-artifact", attack3Base(t, dir))
		if code != 2 || r.Recall != nil || len(r.Missed) != 0 || !strings.Contains(stdout, "selected phase was not examined") {
			t.Errorf("%s: exit=%d recall=%v missed=%v: the command exited 101 and ran no test\n%s", name, code, r.Recall, r.Missed, stdout)
		}
	}
}

// The same command exiting 101 because a crate failed a test (a FAILED result
// in the log) did run, whatever else it holds: the rule must not over-fire.
func TestASelectedCargoCommandWhereOneCrateFailsATestAndAnotherPassesIsExamined(t *testing.T) {
	dir := phaseDir(t, selectedCa, cargoCdFailed, goPass, finished(101))
	selectedStatus(t, dir, selectedCa, ExitInfo{Code: 101, Detail: "exit status 101"})
	writeFile(t, dir, "selected-rust.log", cargoCaPassed+cargoCdFailed)
	code, stdout, r := auditIn(t, dir, "--base-artifact", attack3Base(t, dir))
	if code != 0 || r.Recall == nil || *r.Recall != 1 || len(r.SelectedNotReproduced) != 0 {
		t.Fatalf("exit=%d recall=%v not_reproduced=%v: cd failed a test in the fast phase and in the full suite\n%s", code, r.Recall, r.SelectedNotReproduced, stdout)
	}
}

func goSelected(pkgs ...string) Selected {
	c := newCommand("go", append([]string{"go", "test", "-count=1", "-json"}, pkgs...)...)
	c.GoJSON, c.Log = true, "selected-go.jsonl"
	var units []Unit
	for _, p := range pkgs {
		units = append(units, Unit{Kind: "go", Pkg: p})
	}
	return Selected{Command: c, Units: units}
}

func goPlan(sel Selected) Plan {
	return Plan{Schema: planSchema, Base: baseSHA,
		Full:     []Command{newCommand("rust", "cargo", "test", "--locked", "--workspace", "--no-fail-fast")},
		Selected: []Selected{sel}}
}

const rustOK = "     Running unittests src/lib.rs (t/l)\n\nrunning 0 tests\n\ntest result: ok. 0 passed; 0 failed\n"

// A selected go command whose package could not be listed or built exits 1
// with a final action for the packages it did run and none for that one. The
// package ran nothing: its failure is not inside an examined selection.
func TestASelectedGoCommandWithNoFinalActionForAPackageRanNothingForItAndYieldsNoVerdict(t *testing.T) {
	plan := goPlan(goSelected("toy/ok", "toy/gone"))
	fullGo := `{"Action":"fail","Package":"toy/gone","Test":"TestX"}` + "\n" + `{"Action":"fail","Package":"toy/gone"}` + "\n"
	full := &phaseStatus{Schema: statusSchema, Commands: []ExitInfo{
		{Cmd: "cargo", Log: "full-rust.log", Code: 0, Completed: true, Detail: "exit status 0"},
		{Cmd: "go", Log: "full-go.jsonl", Code: 1, Completed: true, Detail: "exit status 1"},
	}}
	dir := phaseDir(t, plan, rustOK, fullGo, full)
	selectedStatus(t, dir, plan, ExitInfo{Code: 1, Detail: "exit status 1"})
	writeFile(t, dir, "selected-go.jsonl", goPass)
	code, stdout, r := auditIn(t, dir, "--base-artifact", attack3Base(t, dir))
	if code != 2 || r.Recall != nil || len(r.Missed) != 0 {
		t.Fatalf("exit=%d recall=%v missed=%v: toy/gone has no final action in the fast phase's log\n%s", code, r.Recall, r.Missed, stdout)
	}
}

// Whichever shape Go prints for a package that fails to build, the package has
// a final action or it has none. With a final fail action it is examined (and
// failed); as raw text it is no JSON at all, and nothing is examined.
func TestAGoBuildFailureIsExaminedAsAFailureOrNotExaminedWhicheverShapeGoPrints(t *testing.T) {
	const jsonShape = `{"ImportPath":"toy/bad [toy/bad.test]","Action":"build-output","Output":"bad.go:3: syntax error\n"}` + "\n" +
		`{"ImportPath":"toy/bad [toy/bad.test]","Action":"build-fail"}` + "\n" +
		`{"Action":"start","Package":"toy/bad"}` + "\n" +
		`{"Action":"fail","Package":"toy/bad","FailedBuild":"toy/bad [toy/bad.test]"}` + "\n"
	run := parseGoJSON(strings.NewReader(jsonShape))
	if !run.Examined || !run.Done["toy/bad"] || len(run.Failures) != 1 || run.Failures[0].Name != "[package]" {
		t.Errorf("json shape: %+v", run)
	}
	const rawShape = "# toy/bad [toy/bad.test]\nbad.go:3: syntax error\nFAIL\ttoy/bad [build failed]\n"
	if run := parseGoJSON(strings.NewReader(rawShape)); run.Examined || run.Done["toy/bad"] {
		t.Errorf("raw shape: %+v", run)
	}
	// Mixed: another package's events beside the raw text of a failed build.
	mixed := goPass + rawShape
	if run := parseGoJSON(strings.NewReader(mixed)); run.Examined {
		t.Errorf("mixed: %+v", run)
	}
}

// Held, recorded so the report is reproducible: a mixed run where Rust is
// examined and Go is not never reaches a verdict.
func TestMixedExaminedRustUnexaminedGoReachesNoVerdict(t *testing.T) {
	full := &phaseStatus{Schema: statusSchema, Commands: []ExitInfo{
		{Cmd: "cargo", Log: "full-rust.log", Code: 101, Completed: true, Detail: "exit status 101"},
		{Cmd: "go", Log: "full-go.jsonl", Detail: "signal: killed"},
	}}
	dir := phaseDir(t, selectedCa, plantedMiss, `{"Action":"start","Package":"toy/x"}`+"\n", full)
	code, stdout, r := auditIn(t, dir, "--base-artifact", attack3Base(t, dir))
	requireNoVerdict(t, code, stdout, r)
}

// Two declared test targets may share one path (cargo warns, builds both). The
// selection is the crate, so both run in the one command.
func TestTwoDeclaredTargetsOnOnePathAreSelectedWithTheirCrateInOneCommand(t *testing.T) {
	in := inputs(withChanged("rust/ca/tests/shared.rs"), withTests("rust/ca/tests/shared.rs"))
	in.Rust = &RustWorkspace{Packages: []RustPackage{{Name: "ca", Dir: "rust/ca", Targets: []RustTarget{
		{Name: "ca", Kinds: []string{"lib"}, Src: "rust/ca/src/lib.rs", Test: true},
		{Name: "one", Kinds: []string{"test"}, Src: "rust/ca/tests/shared.rs", Test: true},
		{Name: "two", Kinds: []string{"test"}, Src: "rust/ca/tests/shared.rs", Test: true},
	}}}}
	p := BuildPlan(in)
	if len(p.Selected) != 1 || p.Selected[0].Display != "cargo test --locked -p ca --no-fail-fast" {
		t.Fatalf("selected %v: the crate, whole, runs every target that compiles tests/shared.rs", cmds(p))
	}
}
