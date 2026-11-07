package main

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func openFixture(t *testing.T, name string) *os.File {
	t.Helper()
	f, err := os.Open(filepath.Join("testdata", name))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { f.Close() })
	return f
}

func keys(fs []Failure) []string {
	var out []string
	for _, f := range fs {
		out = append(out, f.Key())
	}
	return out
}

func contains(list []string, want string) bool {
	for _, s := range list {
		if s == want {
			return true
		}
	}
	return false
}

// libtest-two-packages.txt and gotest-json.jsonl are real output of `cargo test --workspace
// --no-fail-fast` and `go test -json ./...` over toy workspaces with planted
// failures, with the machine's paths scrubbed.
func TestLibtestFailuresAreAttributedByCargosFooterNotByTheRunningHeader(t *testing.T) {
	run := parseLibtest(openFixture(t, "libtest-two-packages.txt"))
	if !run.Examined {
		t.Fatalf("a log with test results was not examined: %s", run.Note)
	}
	want := []string{
		"rust:ca:--doc:ca/src/lib.rs - d (line 3)",
		"rust:ca:--lib:t::lib_bad",
		"rust:ca:--test foo:integ_bad",
		"rust:cb:--test it:m::in_m",
	}
	if got := keys(run.Failures); strings.Join(got, "\n") != strings.Join(want, "\n") {
		t.Fatalf("failures =\n%s\nwant\n%s", strings.Join(got, "\n"), strings.Join(want, "\n"))
	}
}

func TestALibtestFailureWithNoFooterIsKeptAndUnattributed(t *testing.T) {
	run := parseLibtest(strings.NewReader("     Running tests/x.rs (t/debug/deps/x-1)\n\nrunning 1 test\ntest boom ... FAILED\n\ntest result: FAILED. 0 passed; 1 failed\n"))
	if got := keys(run.Failures); len(got) != 1 || got[0] != "rust:?::boom" {
		t.Fatalf("failures = %v", got)
	}
}

func TestALibtestLogThatNeverRanATestIsNotExamined(t *testing.T) {
	for name, log := range map[string]string{
		"empty":        "",
		"compile only": "   Compiling a v0.1.0\nerror: could not compile `a` (lib) due to 1 previous error\n",
		"compile after a pass": "     Running tests/x.rs (t/x)\ntest result: ok. 1 passed\n" +
			"error: could not compile `b` (lib test) due to 1 previous error\n",
	} {
		run := parseLibtest(strings.NewReader(log))
		if run.Examined || run.Note == "" {
			t.Errorf("%s: examined=%t note=%q; a suite that did not fully run is not one that passed", name, run.Examined, run.Note)
		}
	}
}

func TestAGoLogNamesTheFailingTestsAndAPackageThatFailedWithoutOne(t *testing.T) {
	run := parseGoJSON(openFixture(t, "gotest-json.jsonl"))
	if !run.Examined {
		t.Fatalf("not examined: %s", run.Note)
	}
	got := keys(run.Failures)
	for _, want := range []string{"go:toy/bad:TestBroken", "go:toy/bad:TestBroken/sub", "go:toy/broken:[package]"} {
		if !contains(got, want) {
			t.Errorf("failures %v lack %s", got, want)
		}
	}
	for _, not := range []string{"go:toy/ok:TestFine", "go:toy/bad:TestFine", "go:toy/bad:[package]"} {
		if contains(got, not) {
			t.Errorf("failures %v name %s, which did not fail by itself", got, not)
		}
	}
}

func TestAGoLogThatIsTruncatedOrEmptyIsNotExamined(t *testing.T) {
	for name, log := range map[string]string{"empty": "", "garbage": "FAIL toy [build failed]\n", "cut": `{"Action":"fail","Package":"p","Test":"TestA"}` + "\n{\"Action\":"} {
		run := parseGoJSON(strings.NewReader(log))
		if run.Examined || run.Note == "" || len(run.Failures) != 0 {
			t.Errorf("%s: %+v", name, run)
		}
	}
}

func examined(fs ...Failure) SuiteRun { return SuiteRun{Examined: true, Failures: fs} }

var (
	failCbIt  = Failure{Kind: "rust", Pkg: "cb", Target: "--test it", Name: "m::in_m"}
	failCaFoo = Failure{Kind: "rust", Pkg: "ca", Target: "--test foo", Name: "integ_bad"}
	failCdLib = Failure{Kind: "rust", Pkg: "cd", Target: "--lib", Name: "engine::t::bad"}
	// selectedCa selects crates ca and cd, whole, in one cargo command.
	selectedCa = Plan{Schema: planSchema, Base: baseSHA,
		Full: []Command{newCommand("rust", "cargo", "test", "--locked", "--workspace", "--no-fail-fast")},
		Selected: []Selected{
			{Command: logged(newCommand("rust", "cargo", "test", "--locked", "-p", "ca", "-p", "cd", "--no-fail-fast")), Units: []Unit{{Kind: "rust", Pkg: "ca"}, {Kind: "rust", Pkg: "cd"}}},
		}}
)

func TestASuiteThatWasNotExaminedNeverLooksLikeOneThatPassed(t *testing.T) {
	in := AuditInput{Plan: selectedCa, Rust: SuiteRun{Note: "no test ran"}, Go: examined()}
	r, notes := Audit(in)
	if r.Full.Rust.Examined || r.Full.Rust.Note != "no test ran" || !r.Full.Go.Examined {
		t.Errorf("full = %+v", r.Full)
	}
	if !contains(notes, "::warning::the full Rust suite was not examined: no test ran") {
		t.Errorf("notes = %q", notes)
	}
	if code := Exit(in, r); code != 2 {
		t.Errorf("exit = %d, want 2: a suite that was not examined is not one that passed", code)
	}
}

const mainRef = "refs/heads/main"

func TestQualifyDemotesASuiteItsExitStatusContradicts(t *testing.T) {
	ran := func(code int) *ExitInfo { return &ExitInfo{Code: code, Completed: true, Detail: "exit status"} }
	for name, tc := range map[string]struct {
		run  SuiteRun
		st   *ExitInfo
		want bool
	}{
		"clean pass":                  {examined(), ran(0), true},
		"failures and exit 101":       {examined(failCbIt), ran(101), true},
		"no status":                   {examined(), nil, false},
		"killed":                      {examined(failCbIt), &ExitInfo{Code: -1, Detail: "signal: killed"}, false},
		"exit 101 and nothing parsed": {examined(), ran(101), false},
		"exit 0 with failures":        {examined(failCbIt), ran(0), false},
	} {
		got := Qualify(tc.run, tc.st)
		if got.Examined != tc.want || (!tc.want && got.Note == "") {
			t.Errorf("%s: examined=%t note=%q, want examined=%t", name, got.Examined, got.Note, tc.want)
		}
	}
	if got := Qualify(SuiteRun{Note: "already not examined"}, ran(0)); got.Examined || got.Note != "already not examined" {
		t.Errorf("an unexamined suite was altered: %+v", got)
	}
}

func TestASelectedPhaseRedTheFullSuiteDidNotHaveIsNamed(t *testing.T) {
	in := AuditInput{Plan: selectedCa, Rust: examined(failCaFoo), Go: examined(), SelectedKnown: true,
		SelectedRust: examined(failCaFoo, failCdLib), SelectedGo: examined()}
	r, notes := Audit(in)
	if len(r.SelectedPhaseFailedOnly) != 1 || r.SelectedPhaseFailedOnly[0] != failCdLib.Key() {
		t.Fatalf("selected_phase_failed_only = %v", r.SelectedPhaseFailedOnly)
	}
	if !contains(notes, annotate("warning", "failed in the selected phase only, not in the full suite: "+failCdLib.Key())) {
		t.Errorf("notes = %q", notes)
	}
	in.SelectedKnown = false
	if r, _ := Audit(in); r.SelectedPhaseFailedOnly != nil {
		t.Errorf("with the fast phase unread the field must be null, got %v", r.SelectedPhaseFailedOnly)
	}
	in.SelectedKnown, in.SelectedGo = true, SuiteRun{Note: "cut off"}
	if r, _ := Audit(in); r.SelectedPhaseFailedOnly != nil {
		t.Errorf("with the fast phase unexamined the field must be null, got %v", r.SelectedPhaseFailedOnly)
	}
}

// writeFile puts a fixture under dir and returns its path.
func writeFile(t *testing.T, dir, name, body string) string {
	t.Helper()
	p := filepath.Join(dir, name)
	if err := os.WriteFile(p, []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
	return p
}

// planSHA is the hash run embeds in a status file.
func planSHA(t *testing.T, path string) string {
	t.Helper()
	body, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	sum := sha256.Sum256(body)
	return hex.EncodeToString(sum[:])
}

// phaseDir writes a plan and a full phase's logs and status into one directory.
// The fast phase is recorded as having completed every selected command, as a
// healthy run would leave it; tests that need otherwise overwrite the file.
func phaseDir(t *testing.T, plan Plan, rustLog, goLog string, status *phaseStatus) string {
	t.Helper()
	dir := t.TempDir()
	planPath := filepath.Join(dir, "plan.json")
	if err := writeAtomic(planPath, plan); err != nil {
		t.Fatal(err)
	}
	sha := planSHA(t, planPath)
	if rustLog != "" {
		writeFile(t, dir, "full-rust.log", rustLog)
	}
	if goLog != "" {
		writeFile(t, dir, "full-go.jsonl", goLog)
	}
	if status != nil {
		status.PlanSHA256 = sha
		if err := writeAtomic(filepath.Join(dir, "full-status.json"), *status); err != nil {
			t.Fatal(err)
		}
	}
	sel := phaseStatus{Schema: statusSchema, PlanSHA256: sha}
	for _, s := range plan.Selected {
		// A healthy fast phase left an examined log for each kind it ran.
		if s.GoJSON {
			writeFile(t, dir, "selected-go.jsonl", goPass)
		} else {
			writeFile(t, dir, "selected-rust.log", "     Running unittests src/lib.rs (t/l)\n\nrunning 1 test\ntest ok ... ok\n\ntest result: ok. 1 passed; 0 failed\n")
		}
		sel.Commands = append(sel.Commands, ExitInfo{Cmd: s.Display, Log: s.Log, Completed: true, Detail: "exit status 0"})
	}
	if err := writeAtomic(filepath.Join(dir, "selected-status.json"), sel); err != nil {
		t.Fatal(err)
	}
	return dir
}

func finished(code int) *phaseStatus {
	return &phaseStatus{Schema: statusSchema, Commands: []ExitInfo{
		{Cmd: "cargo", Log: "full-rust.log", Code: code, Completed: true, Detail: "exit status"},
		{Cmd: "go", Log: "full-go.jsonl", Code: 0, Completed: true, Detail: "exit status 0"},
	}}
}

func auditIn(t *testing.T, dir string, extra ...string) (int, string, Recall) {
	t.Helper()
	out := filepath.Join(dir, "selection-recall.json")
	args := append([]string{"audit", "--plan", filepath.Join(dir, "plan.json"), "--sha", "abc", "--event", "push", "--ref", mainRef, "--run-id", "9", "--out", out}, extra...)
	var stdout bytes.Buffer
	code := run(context.Background(), args, &stdout, &bytes.Buffer{})
	var r Recall
	if body, err := os.ReadFile(out); err == nil {
		if err := json.Unmarshal(body, &r); err != nil {
			t.Fatalf("artifact is not JSON: %v\n%s", err, body)
		}
	} else {
		t.Errorf("the artifact was not written (exit %d): %v", code, err)
	}
	return code, stdout.String(), r
}

const plantedMiss = "     Running tests/it.rs (t/it-1)\ntest m::in_m ... FAILED\n\nfailures:\n\n---- m::in_m stdout ----\n\nfailures:\n    m::in_m\n\ntest result: FAILED. 0 passed; 1 failed\n" +
	"error: test failed, to rerun pass `-p cb --test it`\n"

const goPass = `{"Action":"pass","Package":"toy/ok"}` + "\n"

func TestAuditCommandRecordsProvenanceAndTheFullSuitesFailuresAndExitsZero(t *testing.T) {
	dir := phaseDir(t, selectedCa, plantedMiss, goPass, finished(101))
	code, stdout, r := auditIn(t, dir)
	if code != 0 {
		t.Fatalf("exit = %d, want 0: both suites were examined; stdout:\n%s", code, stdout)
	}
	if strings.Contains(stdout, "selection missed") {
		t.Errorf("a verdict about the selection:\n%s", stdout)
	}
	if r.SHA != "abc" || r.Ref != mainRef || r.Event != "push" || r.RunID != "9" || r.Schema != recallSchema {
		t.Errorf("provenance = %s %s %s %s schema %d", r.SHA, r.Ref, r.Event, r.RunID, r.Schema)
	}
	if len(r.Full.Failed) != 1 || r.Full.Failed[0] != "rust:cb:--test it:m::in_m" || !r.Full.Rust.Examined || !r.Full.Go.Examined {
		t.Errorf("full = %+v", r.Full)
	}
}

func TestAuditCommandWithMissingLogsWritesTheArtifactAndExitsTwo(t *testing.T) {
	dir := phaseDir(t, selectedCa, "", "", nil)
	code, stdout, r := auditIn(t, dir)
	if code != 2 || r.Full.Rust.Examined || r.Full.Go.Examined {
		t.Fatalf("exit=%d full=%+v stdout:\n%s", code, r.Full, stdout)
	}
}

func TestAuditOfATruncatedFullSuiteExitsTwo(t *testing.T) {
	// Passing logs, but no status file: nothing says the commands finished.
	dir := phaseDir(t, selectedCa, "     Running tests/x.rs (t/x)\n\nrunning 1 test\ntest a ... ok\n\ntest result: ok. 1 passed\n", goPass, nil)
	code, _, r := auditIn(t, dir)
	if code != 2 || r.Full.Rust.Examined {
		t.Fatalf("exit=%d rust=%+v", code, r.Full.Rust)
	}
	// A status file that says the command was killed.
	killed := finished(0)
	killed.Commands[0] = ExitInfo{Cmd: "cargo", Log: "full-rust.log", Code: -1, Detail: "signal: killed"}
	dir = phaseDir(t, selectedCa, "     Running tests/x.rs (t/x)\n\nrunning 1 test\ntest a ... ok\n\ntest result: ok. 1 passed\n", goPass, killed)
	if code, _, r := auditIn(t, dir); code != 2 || r.Full.Rust.Examined || !strings.Contains(r.Full.Rust.Note, "did not run to completion") {
		t.Fatalf("killed: exit=%d rust=%+v", code, r.Full.Rust)
	}
	// Exit 101 with no failing test in the log.
	dir = phaseDir(t, selectedCa, "     Running tests/x.rs (t/x)\n\nrunning 1 test\ntest a ... ok\n\ntest result: ok. 1 passed\n", goPass, finished(101))
	if code, _, r := auditIn(t, dir); code != 2 || r.Full.Rust.Examined {
		t.Fatalf("exit 101 with nothing parsed: exit=%d rust=%+v", code, r.Full.Rust)
	}
	// A clean, finished run is examined and exits 0.
	dir = phaseDir(t, selectedCa, "     Running tests/x.rs (t/x)\n\nrunning 1 test\ntest a ... ok\n\ntest result: ok. 1 passed\n", goPass, finished(0))
	if code, _, r := auditIn(t, dir); code != 0 || !r.Full.Rust.Examined {
		t.Fatalf("clean: exit=%d rust=%+v", code, r.Full.Rust)
	}
}

func TestAuditCommandWithAnUnusablePlanStillWritesTheArtifact(t *testing.T) {
	dir := phaseDir(t, selectedCa, "", "", nil)
	if err := os.WriteFile(filepath.Join(dir, "plan.json"), []byte("{broken"), 0o644); err != nil {
		t.Fatal(err)
	}
	code, stdout, r := auditIn(t, dir)
	if code != 2 || r.Full.Rust.Examined || !strings.Contains(stdout, "::error::the plan was unusable") {
		t.Fatalf("exit=%d rust=%+v stdout:\n%s", code, r.Full.Rust, stdout)
	}
	// And with no plan at all.
	if err := os.Remove(filepath.Join(dir, "plan.json")); err != nil {
		t.Fatal(err)
	}
	if code, _, r := auditIn(t, dir); code != 2 || r.Schema != recallSchema {
		t.Fatalf("no plan: exit=%d schema=%d", code, r.Schema)
	}
}

func TestTheFastPhasesLogsNameASelectedOnlyFailure(t *testing.T) {
	plan := selectedCa
	plan.Selected = []Selected{{Command: logged(newCommand("rust", "cargo", "test")), Units: []Unit{{Kind: "rust", Pkg: "ca"}}}}
	dir := phaseDir(t, plan, "     Running tests/x.rs (t/x)\n\nrunning 1 test\ntest a ... ok\n\ntest result: ok. 1 passed\n", goPass, finished(0))
	writeFile(t, dir, "selected-rust.log", "     Running unittests src/lib.rs (t/l)\n\nrunning 1 test\ntest only_here ... FAILED\n\nfailures:\n\n---- only_here stdout ----\n\nfailures:\n    only_here\n\ntest result: FAILED. 0 passed; 1 failed\nerror: test failed, to rerun pass `-p ca --lib`\n")
	_, stdout, r := auditIn(t, dir)
	if len(r.SelectedPhaseFailedOnly) != 1 || r.SelectedPhaseFailedOnly[0] != "rust:ca:--lib:only_here" || !strings.Contains(stdout, "selected phase only") {
		t.Fatalf("selected_phase_failed_only=%v stdout:\n%s", r.SelectedPhaseFailedOnly, stdout)
	}
}

func TestAPlanWithNoFullCommandExitsTwoThroughTheCommand(t *testing.T) {
	dir := phaseDir(t, selectedCa, plantedMiss, goPass, finished(101))
	if err := os.WriteFile(filepath.Join(dir, "plan.json"), []byte(`{"schema":1}`), 0o644); err != nil {
		t.Fatal(err)
	}
	code, stdout, _ := auditIn(t, dir)
	if code != 2 {
		t.Fatalf("exit = %d, want 2; stdout:\n%s", code, stdout)
	}
	if strings.Contains(stdout, "selection missed") {
		t.Errorf("a verdict about a selection nobody knows:\n%s", stdout)
	}
}

func TestAPlanWithNoFullCommandOrAnUnknownSchemaIsRefused(t *testing.T) {
	dir := t.TempDir()
	for name, body := range map[string]string{
		"no full":         `{"schema":1}`,
		"empty full":      `{"schema":1,"full":[]}`,
		"unknown schema":  `{"schema":9,"full":[{"cmd":"x","dir":".","argv":["x"]}]}`,
		"command no argv": `{"schema":1,"full":[{"cmd":"x","dir":".","argv":[]}]}`,
	} {
		p := writeFile(t, dir, name+".json", body)
		if _, _, err := loadPlan(p); err == nil {
			t.Errorf("%s: accepted", name)
		}
	}
}

func TestAFastPhaseThatDidNotRunExitsTwo(t *testing.T) {
	failCd := cargoCdFailed
	// The fast phase's cargo command was killed: nothing says it ran.
	dir := phaseDir(t, selectedCa, failCd, goPass, finished(101))
	killed := phaseStatus{Schema: statusSchema, PlanSHA256: planSHA(t, filepath.Join(dir, "plan.json")), Commands: []ExitInfo{
		{Cmd: selectedCa.Selected[0].Display, Log: "selected-rust.log", Code: -1, Detail: "signal: killed"},
	}}
	if err := writeAtomic(filepath.Join(dir, "selected-status.json"), killed); err != nil {
		t.Fatal(err)
	}
	code, stdout, _ := auditIn(t, dir)
	if code != 2 || !strings.Contains(stdout, "the selected phase was not examined") {
		t.Fatalf("killed: exit=%d\n%s", code, stdout)
	}
	// And with no selected status at all.
	dir = phaseDir(t, selectedCa, failCd, goPass, finished(101))
	if err := os.Remove(filepath.Join(dir, "selected-status.json")); err != nil {
		t.Fatal(err)
	}
	code, stdout, _ = auditIn(t, dir)
	if code != 2 || !strings.Contains(stdout, "no usable fast-phase status") {
		t.Fatalf("missing: exit=%d\n%s", code, stdout)
	}
	// A healthy fast phase that failed the same test as the full suite is examined: exit 0.
	dir = phaseDir(t, selectedCa, failCd, goPass, finished(101))
	selectedStatus(t, dir, selectedCa, ExitInfo{Code: 101, Detail: "exit status 101"})
	writeFile(t, dir, "selected-rust.log", failCd)
	if code, _, r := auditIn(t, dir); code != 0 || r.SelectedPhaseFailedOnly == nil || len(r.SelectedPhaseFailedOnly) != 0 {
		t.Fatalf("healthy: exit=%d selected_only=%v", code, r.SelectedPhaseFailedOnly)
	}
}

func TestAStatusFromAnotherPlanIsRefused(t *testing.T) {
	log := "     Running tests/x.rs (t/x)\n\nrunning 1 test\ntest a ... ok\n\ntest result: ok. 1 passed\n"
	dir := phaseDir(t, selectedCa, log, goPass, finished(0))
	st, err := os.ReadFile(filepath.Join(dir, "full-status.json"))
	if err != nil {
		t.Fatal(err)
	}
	stale := strings.Replace(string(st), `"plan_sha256": "`, `"plan_sha256": "0000`, 1)
	if stale == string(st) {
		t.Fatal("fixture has no plan hash to alter")
	}
	writeFile(t, dir, "full-status.json", stale)
	code, _, r := auditIn(t, dir)
	if code != 2 || r.Full.Rust.Examined || !strings.Contains(r.Full.Rust.Note, "stale status") {
		t.Fatalf("exit=%d rust=%+v", code, r.Full.Rust)
	}
}
