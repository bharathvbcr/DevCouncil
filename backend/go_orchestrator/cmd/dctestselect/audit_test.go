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
	failGoBad = Failure{Kind: "go", Pkg: "toy/bad", Name: "TestBroken"}
	// selectedCa selects ca's `foo` test target and the whole of crate cd.
	selectedCa = Plan{Schema: planSchema, Base: baseSHA,
		Full: []Command{newCommand("rust", "cargo", "test", "--locked", "--workspace", "--no-fail-fast")},
		Selected: []Selected{
			{Command: newCommand("rust", "cargo", "test", "-p", "ca", "--test", "foo"), Units: []Unit{{Kind: "rust", Pkg: "ca", Target: "--test foo"}}},
			{Command: newCommand("rust", "cargo", "test", "-p", "cd"), Units: []Unit{{Kind: "rust", Pkg: "cd"}}},
		}}
)

func TestAFailureTheSelectionDidNotRunAndTheBaseDidNotHaveIsAMiss(t *testing.T) {
	r, notes := Audit(AuditInput{Plan: selectedCa, SHA: "s", Rust: examined(failCaFoo, failCbIt), Go: examined(), Base: []string{}})
	if len(r.Missed) != 1 || r.Missed[0] != failCbIt.Key() {
		t.Fatalf("missed = %v", r.Missed)
	}
	if !contains(notes, "::error::selection missed "+failCbIt.Key()) {
		t.Errorf("notes = %q; want an error naming the test", notes)
	}
	if r.NFailed != 2 || r.Recall == nil || *r.Recall != 0.5 {
		t.Errorf("n_failed=%d recall=%v; want 2 and 0.5", r.NFailed, r.Recall)
	}
	if !contains(r.Full.Failed, failCaFoo.Key()) || !contains(r.Full.Failed, failCbIt.Key()) {
		t.Errorf("full.failed = %v", r.Full.Failed)
	}
}

func TestAFailureTheBaseAlreadyHadIsNotAMissAndIsNotCounted(t *testing.T) {
	r, notes := Audit(AuditInput{Plan: selectedCa, Rust: examined(failCbIt, failCaFoo), Go: examined(), Base: []string{failCbIt.Key()}})
	if len(r.Missed) != 0 {
		t.Fatalf("a failure the base had was reported as a miss: %v", r.Missed)
	}
	for _, n := range notes {
		if strings.HasPrefix(n, "::error::") {
			t.Errorf("unexpected error annotation %q", n)
		}
	}
	if r.NFailed != 1 || r.Recall == nil || *r.Recall != 1 {
		t.Errorf("n_failed=%d recall=%v; want only the new failure, found by the selection", r.NFailed, r.Recall)
	}
	if len(r.BaseFailed) != 1 {
		t.Errorf("base_failed = %v", r.BaseFailed)
	}
}

func TestRecallIsNullWhenNothingFailedNeverOne(t *testing.T) {
	r, _ := Audit(AuditInput{Plan: selectedCa, Rust: examined(), Go: examined(), Base: []string{}})
	if r.Recall != nil {
		t.Fatalf("recall = %v with nothing failed; it must be null, not 1.0", *r.Recall)
	}
	body, err := json.Marshal(r)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Contains(body, []byte(`"recall":null`)) || !bytes.Contains(body, []byte(`"n_failed":0`)) {
		t.Errorf("artifact = %s", body)
	}
	// And when only the base's failures failed, too.
	r, _ = Audit(AuditInput{Plan: selectedCa, Rust: examined(failCbIt), Go: examined(), Base: []string{failCbIt.Key()}})
	if r.Recall != nil || r.NFailed != 0 {
		t.Errorf("recall=%v n_failed=%d when every failure was the base's", r.Recall, r.NFailed)
	}
}

func TestWithTheBaseUnknownAFailureOutsideTheSelectionIsUnclassifiedNotAMiss(t *testing.T) {
	r, notes := Audit(AuditInput{Plan: selectedCa, Rust: examined(failCbIt), Go: examined(), Base: nil, BaseWhy: "no base-branch artifact was given"})
	if r.BaseFailed != nil {
		t.Errorf("base_failed = %v; unknown must be null", r.BaseFailed)
	}
	if len(r.Missed) != 0 || len(r.Unclassified) != 1 {
		t.Errorf("missed=%v unclassified=%v", r.Missed, r.Unclassified)
	}
	if r.Recall != nil {
		t.Errorf("recall = %v; with the base unknown it cannot be asserted", *r.Recall)
	}
	var warned bool
	for _, n := range notes {
		if strings.HasPrefix(n, "::error::") {
			t.Errorf("an unknown base must warn, not error: %q", n)
		}
		warned = warned || strings.HasPrefix(n, "::warning::base_failed is unknown")
	}
	if !warned {
		t.Errorf("notes = %q; want a base_failed warning", notes)
	}
	body, _ := json.Marshal(r)
	if !bytes.Contains(body, []byte(`"base_failed":null`)) {
		t.Errorf("artifact = %s", body)
	}
}

func TestASuiteThatWasNotExaminedNeverLooksLikeOneThatPassed(t *testing.T) {
	r, notes := Audit(AuditInput{Plan: selectedCa, Rust: SuiteRun{Note: "no test ran"}, Go: examined(), Base: []string{}})
	if r.Recall != nil || r.Full.Rust.Examined || r.RecallNote == "" {
		t.Errorf("recall=%v rust=%+v note=%q", r.Recall, r.Full.Rust, r.RecallNote)
	}
	if !contains(notes, "::warning::the full Rust suite was not examined: no test ran") {
		t.Errorf("notes = %q", notes)
	}
}

func TestAnEverythingPlanCoversEveryFailureButSaysNothingAboutRecall(t *testing.T) {
	r, _ := Audit(AuditInput{Plan: Plan{Everything: true}, Rust: examined(failCbIt), Go: examined(failGoBad), Base: []string{}})
	if len(r.Missed) != 0 || r.NFailed != 2 {
		t.Fatalf("missed=%v n_failed=%d", r.Missed, r.NFailed)
	}
	if r.Recall != nil || !strings.Contains(r.RecallNote, "everything") {
		t.Fatalf("recall=%v note=%q; a widened run must not report a recall of 1.0", r.Recall, r.RecallNote)
	}
}

func TestACoveredFailureMatchesPackageAndTargetOnly(t *testing.T) {
	if !selectedCa.Covers(failCaFoo) || !selectedCa.Covers(failCdLib) {
		t.Fatal("a failure in a selected target, or anywhere in a wholly selected crate, is covered")
	}
	for name, f := range map[string]Failure{
		"another package": failCbIt,
		"another target":  {Kind: "rust", Pkg: "ca", Target: "--test bar", Name: "x"},
		"the lib of a crate whose only selected target is a test": {Kind: "rust", Pkg: "ca", Target: "--lib", Name: "x"},
		"a doctest of the same":                                   {Kind: "rust", Pkg: "ca", Target: "--doc", Name: "ca/src/lib.rs - d (line 3)"},
		"go":                                                      failGoBad,
	} {
		if selectedCa.Covers(f) {
			t.Errorf("%s was reported as covered", name)
		}
	}
	// A module path never narrows a unit: the name of the test is not consulted.
	if !selectedCa.Covers(Failure{Kind: "rust", Pkg: "cd", Target: "--lib", Name: "db::payload_serialization_tests::roundtrip"}) {
		t.Error("a test of a wholly selected crate is covered whatever its module path")
	}
	whole := Plan{Selected: []Selected{{Units: []Unit{{Kind: "rust", Pkg: "ca"}, {Kind: "rust", Pkg: "cd"}, {Kind: "go", Pkg: "toy/bad"}}}}}
	if !whole.Covers(failCaFoo) || !whole.Covers(failCdLib) || !whole.Covers(failGoBad) {
		t.Error("a unit with no target covers every target of its package, and a Go package covers its tests")
	}
}

const mainRef = "refs/heads/main"

func baseDoc(mutate func(string) string) []byte {
	return []byte(mutate(`{"schema":2,"sha":"abc","ref":"refs/heads/main","event":"push","run_id":"7","full":{"rust":{"examined":true},"go":{"examined":true},"failed":["a","b"]}}`))
}

func TestBaseFailuresAcceptsOnlyAnExaminedSameSchemaPushRecordOfTheBaseBranch(t *testing.T) {
	got, rec, why := BaseFailures(baseDoc(func(s string) string { return s }), mainRef)
	if len(got) != 2 || why != "" || rec.SHA != "abc" || rec.RunID != "7" {
		t.Fatalf("got=%v rec=%+v why=%q", got, rec, why)
	}
	empty := baseDoc(func(s string) string { return strings.Replace(s, `"failed":["a","b"]`, `"failed":[]`, 1) })
	if got, _, why := BaseFailures(empty, mainRef); got == nil || why != "" {
		t.Fatalf("an examined base with no failures is an empty list, not unknown: got=%v why=%q", got, why)
	}
	for name, mutate := range map[string]func(string) string{
		"other schema":   func(s string) string { return strings.Replace(s, `"schema":2`, `"schema":1`, 1) },
		"other ref":      func(s string) string { return strings.Replace(s, `refs/heads/main`, `refs/pull/9/merge`, 1) },
		"no ref":         func(s string) string { return strings.Replace(s, `"ref":"refs/heads/main",`, ``, 1) },
		"pull request":   func(s string) string { return strings.Replace(s, `"event":"push"`, `"event":"pull_request"`, 1) },
		"no failed list": func(s string) string { return strings.Replace(s, `,"failed":["a","b"]`, ``, 1) },
		"null failed":    func(s string) string { return strings.Replace(s, `["a","b"]`, `null`, 1) },
		"base unexamined": func(s string) string {
			return strings.Replace(s, `"rust":{"examined":true}`, `"rust":{"examined":false}`, 1)
		},
		"not json": func(string) string { return "nope" },
	} {
		if got, _, why := BaseFailures(baseDoc(mutate), mainRef); got != nil || why == "" {
			t.Errorf("%s: got=%v why=%q; an untrustworthy base must be unknown", name, got, why)
		}
	}
}

func TestADifferentBaseCommitIsNotedAndRecorded(t *testing.T) {
	plan := Plan{Base: "planbase"}
	base := []string{}
	r, notes := Audit(AuditInput{Plan: plan, Rust: examined(), Go: examined(), Base: base, BaseInfo: &BaseRecord{SHA: "other", RunID: "7", Ref: mainRef, Event: "push"}})
	if r.BaseRecord == nil || r.BaseRecord.MatchesPlanBase || r.BaseRecord.SHA != "other" || r.BaseRecord.RunID != "7" {
		t.Fatalf("base_record = %+v", r.BaseRecord)
	}
	if !contains(notes, annotate("notice", "the base record is from other (run 7) and the plan measured from planbase; a test fixed or broken between them is misattributed")) {
		t.Errorf("notes = %q", notes)
	}
	r, notes = Audit(AuditInput{Plan: plan, Rust: examined(), Go: examined(), Base: base, BaseInfo: &BaseRecord{SHA: "planbase"}})
	if !r.BaseRecord.MatchesPlanBase || len(notes) != 0 {
		t.Errorf("matching base: %+v notes=%q", r.BaseRecord, notes)
	}
}

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
	in := AuditInput{Plan: selectedCa, Rust: examined(failCaFoo), Go: examined(), Base: []string{}, SelectedKnown: true,
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
	if err := writeJSON(planPath, plan); err != nil {
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
		if err := writeJSON(filepath.Join(dir, "full-status.json"), *status); err != nil {
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
	if err := writeJSON(filepath.Join(dir, "selected-status.json"), sel); err != nil {
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

func TestAuditCommandEndToEndReportsAPlantedMissAndExitsOne(t *testing.T) {
	dir := phaseDir(t, selectedCa, plantedMiss, goPass, finished(101))
	// The base had a different failure, so the planted one is new.
	base := writeFile(t, dir, "base.json", `{"schema":2,"sha":"bb","ref":"refs/heads/main","event":"push","run_id":"1","full":{"rust":{"examined":true},"go":{"examined":true},"failed":["rust:cb:--test it:old"]}}`)
	code, stdout, r := auditIn(t, dir, "--base-artifact", base)
	if code != 1 {
		t.Fatalf("exit = %d, want 1; stdout:\n%s", code, stdout)
	}
	if !strings.Contains(stdout, "::error::selection missed rust:cb:--test it:m::in_m") {
		t.Errorf("stdout = %q", stdout)
	}
	if r.SHA != "abc" || r.Ref != mainRef || r.Event != "push" || r.RunID != "9" || r.Schema != 2 {
		t.Errorf("provenance = %s %s %s %s schema %d", r.SHA, r.Ref, r.Event, r.RunID, r.Schema)
	}
	if len(r.Missed) != 1 || r.Recall == nil || *r.Recall != 0 || r.NFailed != 1 || r.BaseRecord == nil || r.BaseRecord.RunID != "1" {
		t.Errorf("artifact = %+v", r)
	}
}

func TestAuditCommandWithMissingLogsWritesTheArtifactAndExitsTwo(t *testing.T) {
	dir := phaseDir(t, selectedCa, "", "", nil)
	code, stdout, r := auditIn(t, dir)
	if code != 2 || r.Recall != nil || r.Full.Rust.Examined || r.Full.Go.Examined || r.BaseFailed != nil {
		t.Fatalf("exit=%d recall=%v full=%+v base=%v stdout:\n%s", code, r.Recall, r.Full, r.BaseFailed, stdout)
	}
}

func TestAuditOfATruncatedFullSuiteExitsTwoWithNullRecall(t *testing.T) {
	// Passing logs, but no status file: nothing says the commands finished.
	dir := phaseDir(t, selectedCa, "     Running tests/x.rs (t/x)\n\nrunning 1 test\ntest a ... ok\n\ntest result: ok. 1 passed\n", goPass, nil)
	code, _, r := auditIn(t, dir)
	if code != 2 || r.Recall != nil || r.Full.Rust.Examined {
		t.Fatalf("exit=%d recall=%v rust=%+v", code, r.Recall, r.Full.Rust)
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
	base := writeFile(t, dir, "base.json", string(baseDoc(func(s string) string { return strings.Replace(s, `["a","b"]`, `[]`, 1) })))
	if code, _, r := auditIn(t, dir, "--base-artifact", base); code != 0 || !r.Full.Rust.Examined || r.Recall != nil {
		t.Fatalf("clean: exit=%d rust=%+v recall=%v", code, r.Full.Rust, r.Recall)
	}
}

func TestAnUnknownBaseWithUnclassifiedFailuresExitsTwo(t *testing.T) {
	dir := phaseDir(t, selectedCa, plantedMiss, goPass, finished(101))
	code, stdout, r := auditIn(t, dir)
	if code != 2 || len(r.Unclassified) != 1 || len(r.Missed) != 0 || r.BaseFailed != nil {
		t.Fatalf("exit=%d unclassified=%v missed=%v stdout:\n%s", code, r.Unclassified, r.Missed, stdout)
	}
}

func TestAuditCommandWithAnUnusablePlanStillWritesTheArtifact(t *testing.T) {
	dir := phaseDir(t, selectedCa, "", "", nil)
	if err := os.WriteFile(filepath.Join(dir, "plan.json"), []byte("{broken"), 0o644); err != nil {
		t.Fatal(err)
	}
	code, stdout, r := auditIn(t, dir)
	if code != 2 || r.Recall != nil || !strings.Contains(r.RecallNote, "plan was unusable") || !strings.Contains(stdout, "::error::the plan was unusable") {
		t.Fatalf("exit=%d note=%q stdout:\n%s", code, r.RecallNote, stdout)
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
	plan.Selected = []Selected{{Command: logged(newCommand("rust", "cargo", "test")), Units: []Unit{{Kind: "rust", Pkg: "ca", Target: "--lib"}}}}
	dir := phaseDir(t, plan, "     Running tests/x.rs (t/x)\n\nrunning 1 test\ntest a ... ok\n\ntest result: ok. 1 passed\n", goPass, finished(0))
	writeFile(t, dir, "selected-rust.log", "     Running unittests src/lib.rs (t/l)\n\nrunning 1 test\ntest only_here ... FAILED\n\nfailures:\n\n---- only_here stdout ----\n\nfailures:\n    only_here\n\ntest result: FAILED. 0 passed; 1 failed\nerror: test failed, to rerun pass `-p ca --lib`\n")
	_, stdout, r := auditIn(t, dir)
	if len(r.SelectedPhaseFailedOnly) != 1 || r.SelectedPhaseFailedOnly[0] != "rust:ca:--lib:only_here" || !strings.Contains(stdout, "selected phase only") {
		t.Fatalf("selected_phase_failed_only=%v stdout:\n%s", r.SelectedPhaseFailedOnly, stdout)
	}
}

func TestAnUnusablePlanDoesNotBlameTheSelection(t *testing.T) {
	dir := phaseDir(t, selectedCa, plantedMiss, goPass, finished(101))
	// The plan is replaced by one with no full-suite command: unusable.
	if err := os.WriteFile(filepath.Join(dir, "plan.json"), []byte(`{"schema":1}`), 0o644); err != nil {
		t.Fatal(err)
	}
	base := writeFile(t, dir, "base.json", string(baseDoc(func(s string) string { return strings.Replace(s, `["a","b"]`, `[]`, 1) })))
	code, stdout, r := auditIn(t, dir, "--base-artifact", base)
	if code != 2 {
		t.Fatalf("exit = %d, want 2; stdout:\n%s", code, stdout)
	}
	if len(r.Missed) != 0 || len(r.Unclassified) != 1 || r.Recall != nil {
		t.Fatalf("missed=%v unclassified=%v recall=%v; with no usable plan every failure is unclassified", r.Missed, r.Unclassified, r.Recall)
	}
	if strings.Contains(stdout, "selection missed") || strings.Contains(stdout, "measured from") {
		t.Errorf("a verdict or a base notice about a selection nobody knows:\n%s", stdout)
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

func TestRecallIsNullWhenASelectedCommandDidNotComplete(t *testing.T) {
	failCd := "     Running unittests src/lib.rs (t/l)\n\nrunning 1 test\ntest engine::t::bad ... FAILED\n\nfailures:\n\n---- engine::t::bad stdout ----\n\nfailures:\n    engine::t::bad\n\ntest result: FAILED. 0 passed; 1 failed\nerror: test failed, to rerun pass `-p cd --lib`\n"
	base := func(dir string) string {
		return writeFile(t, dir, "base.json", string(baseDoc(func(s string) string { return strings.Replace(s, `["a","b"]`, `[]`, 1) })))
	}
	// The fast phase's `-p cd` command was killed: nothing says it ran.
	dir := phaseDir(t, selectedCa, failCd, goPass, finished(101))
	sha := planSHA(t, filepath.Join(dir, "plan.json"))
	killed := phaseStatus{Schema: statusSchema, PlanSHA256: sha, Commands: []ExitInfo{
		{Cmd: selectedCa.Selected[0].Display, Log: "selected-rust.log", Completed: true, Detail: "exit status 0"},
		{Cmd: selectedCa.Selected[1].Display, Log: "selected-rust.log", Code: -1, Detail: "signal: killed"},
	}}
	if err := writeJSON(filepath.Join(dir, "selected-status.json"), killed); err != nil {
		t.Fatal(err)
	}
	code, stdout, r := auditIn(t, dir, "--base-artifact", base(dir))
	if code != 2 || r.Recall != nil || len(r.Missed) != 0 || len(r.Unclassified) != 1 || !strings.Contains(r.RecallNote, "fast phase was not examined") {
		t.Fatalf("killed: exit=%d recall=%v missed=%v unclassified=%v note=%q\n%s", code, r.Recall, r.Missed, r.Unclassified, r.RecallNote, stdout)
	}
	// And with no selected status at all.
	dir = phaseDir(t, selectedCa, failCd, goPass, finished(101))
	if err := os.Remove(filepath.Join(dir, "selected-status.json")); err != nil {
		t.Fatal(err)
	}
	code, _, r = auditIn(t, dir, "--base-artifact", base(dir))
	if code != 2 || r.Recall != nil || len(r.Missed) != 0 || !strings.Contains(r.RecallNote, "not examined") {
		t.Fatalf("missing: exit=%d recall=%v missed=%v note=%q", code, r.Recall, r.Missed, r.RecallNote)
	}
	// A healthy fast phase, with the same failure inside the selection, is a recall of 1.
	dir = phaseDir(t, selectedCa, failCd, goPass, finished(101))
	if code, _, r := auditIn(t, dir, "--base-artifact", base(dir)); code != 0 || r.Recall == nil || *r.Recall != 1 {
		t.Fatalf("healthy: exit=%d recall=%v", code, r.Recall)
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
