package main

import (
	"bytes"
	"context"
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
	run := parseLibtest(strings.NewReader("   Running tests/x.rs (t/debug/deps/x-1)\n\nrunning 1 test\ntest boom ... FAILED\n\ntest result: FAILED. 0 passed; 1 failed\n"))
	if got := keys(run.Failures); len(got) != 1 || got[0] != "rust:?::boom" {
		t.Fatalf("failures = %v", got)
	}
}

func TestALibtestLogThatNeverRanATestIsNotExamined(t *testing.T) {
	for name, log := range map[string]string{
		"empty":        "",
		"compile only": "   Compiling a v0.1.0\nerror: could not compile `a` (lib) due to 1 previous error\n",
		"compile after a pass": "   Running tests/x.rs (t/x)\ntest result: ok. 1 passed\n" +
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
	failCbIt   = Failure{Kind: "rust", Pkg: "cb", Target: "--test it", Name: "m::in_m"}
	failCaFoo  = Failure{Kind: "rust", Pkg: "ca", Target: "--test foo", Name: "integ_bad"}
	failCaLib  = Failure{Kind: "rust", Pkg: "ca", Target: "--lib", Name: "engine::t::bad"}
	failGoBad  = Failure{Kind: "go", Pkg: "toy/bad", Name: "TestBroken"}
	selectedCa = Plan{Schema: planSchema, Base: baseSHA, Selected: []Selected{
		{Command: newCommand("rust", "cargo", "test", "-p", "ca", "--test", "foo"), Units: []Unit{{Kind: "rust", Pkg: "ca", Target: "--test foo"}}},
		{Command: newCommand("rust", "cargo", "test", "-p", "ca", "--lib"), Units: []Unit{{Kind: "rust", Pkg: "ca", Target: "--lib", Filters: []string{"engine::"}}}},
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

func TestAnEverythingPlanCoversEveryFailure(t *testing.T) {
	r, _ := Audit(AuditInput{Plan: Plan{Everything: true}, Rust: examined(failCbIt), Go: examined(failGoBad), Base: []string{}})
	if len(r.Missed) != 0 || r.Recall == nil || *r.Recall != 1 || r.NFailed != 2 {
		t.Fatalf("missed=%v recall=%v n_failed=%d", r.Missed, r.Recall, r.NFailed)
	}
}

func TestACoveredFailureMatchesTargetFilterAndPackage(t *testing.T) {
	if !selectedCa.Covers(failCaFoo) || !selectedCa.Covers(failCaLib) {
		t.Fatal("a failure in a selected target, inside its filter, is covered")
	}
	for name, f := range map[string]Failure{
		"another package":  failCbIt,
		"another target":   {Kind: "rust", Pkg: "ca", Target: "--test bar", Name: "x"},
		"outside a filter": {Kind: "rust", Pkg: "ca", Target: "--lib", Name: "other::t::bad"},
		"a doctest":        {Kind: "rust", Pkg: "ca", Target: "--doc", Name: "ca/src/lib.rs - d (line 3)"},
		"go":               failGoBad,
	} {
		if selectedCa.Covers(f) {
			t.Errorf("%s was reported as covered", name)
		}
	}
	whole := Plan{Selected: []Selected{{Units: []Unit{{Kind: "rust", Pkg: "ca"}, {Kind: "go", Pkg: "toy/bad"}}}}}
	if !whole.Covers(failCaFoo) || !whole.Covers(failCaLib) || !whole.Covers(failGoBad) {
		t.Error("a unit with no target covers every target of its package, and a Go package covers its tests")
	}
}

func TestBaseFailuresAcceptsOnlyAnExaminedSameSchemaRecord(t *testing.T) {
	good := `{"schema":1,"full":{"rust":{"examined":true},"go":{"examined":true},"failed":["a","b"]}}`
	if got, why := BaseFailures([]byte(good)); len(got) != 2 || why != "" {
		t.Fatalf("got=%v why=%q", got, why)
	}
	empty := `{"schema":1,"full":{"rust":{"examined":true},"go":{"examined":true},"failed":[]}}`
	if got, why := BaseFailures([]byte(empty)); got == nil || why != "" {
		t.Fatalf("an examined base with no failures is an empty list, not unknown: got=%v why=%q", got, why)
	}
	for name, doc := range map[string]string{
		"not json":        "nope",
		"other schema":    `{"schema":2,"full":{"rust":{"examined":true},"go":{"examined":true},"failed":[]}}`,
		"no failed list":  `{"schema":1,"full":{"rust":{"examined":true},"go":{"examined":true}}}`,
		"null failed":     `{"schema":1,"full":{"rust":{"examined":true},"go":{"examined":true},"failed":null}}`,
		"base unexamined": `{"schema":1,"full":{"rust":{"examined":false},"go":{"examined":true},"failed":[]}}`,
		"no full":         `{"schema":1}`,
	} {
		if got, why := BaseFailures([]byte(doc)); got != nil || why == "" {
			t.Errorf("%s: got=%v why=%q; an untrustworthy base must be unknown", name, got, why)
		}
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

func TestAuditCommandEndToEndReportsAPlantedMissAndExitsOne(t *testing.T) {
	dir := t.TempDir()
	planPath := filepath.Join(dir, "plan.json")
	if err := writeJSON(planPath, selectedCa); err != nil {
		t.Fatal(err)
	}
	rustLog := writeFile(t, dir, "rust.log",
		"   Running tests/it.rs (t/it-1)\ntest m::in_m ... FAILED\n\ntest result: FAILED. 0 passed; 1 failed\n"+
			"error: test failed, to rerun pass `-p cb --test it`\n")
	goLog := writeFile(t, dir, "go.jsonl", `{"Action":"pass","Package":"toy/ok"}`+"\n")
	// The base had a different failure, so the planted one is new.
	base := writeFile(t, dir, "base.json", `{"schema":1,"full":{"rust":{"examined":true},"go":{"examined":true},"failed":["rust:cb:--test it:old"]}}`)
	out := filepath.Join(dir, "selection-recall.json")
	var stdout bytes.Buffer
	code := run(context.Background(), []string{"audit", "--plan", planPath, "--rust-log", rustLog, "--go-log", goLog,
		"--base-artifact", base, "--sha", "abc", "--out", out}, &stdout, &bytes.Buffer{})
	if code != 1 {
		t.Fatalf("exit = %d, want 1; stdout:\n%s", code, stdout.String())
	}
	if !strings.Contains(stdout.String(), "::error::selection missed rust:cb:--test it:m::in_m") {
		t.Errorf("stdout = %q", stdout.String())
	}
	var r Recall
	body, err := os.ReadFile(out)
	if err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal(body, &r); err != nil {
		t.Fatal(err)
	}
	if r.SHA != "abc" || len(r.Missed) != 1 || r.Recall == nil || *r.Recall != 0 || r.NFailed != 1 {
		t.Errorf("artifact = %s", body)
	}
}

func TestAuditCommandWithAMissingLogWritesTheArtifactAndExitsTwo(t *testing.T) {
	dir := t.TempDir()
	planPath := filepath.Join(dir, "plan.json")
	if err := writeJSON(planPath, selectedCa); err != nil {
		t.Fatal(err)
	}
	out := filepath.Join(dir, "selection-recall.json")
	var stdout bytes.Buffer
	code := run(context.Background(), []string{"audit", "--plan", planPath, "--rust-log", filepath.Join(dir, "absent.log"),
		"--go-log", filepath.Join(dir, "absent.jsonl"), "--out", out}, &stdout, &bytes.Buffer{})
	if code != 2 {
		t.Fatalf("exit = %d, want 2 (could not measure); stdout:\n%s", code, stdout.String())
	}
	body, err := os.ReadFile(out)
	if err != nil {
		t.Fatalf("the artifact must be written even when nothing could be measured: %v", err)
	}
	if !bytes.Contains(body, []byte(`"recall": null`)) || !bytes.Contains(body, []byte(`"base_failed": null`)) {
		t.Errorf("artifact = %s", body)
	}
}
