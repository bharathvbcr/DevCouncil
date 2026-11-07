package main

import (
	"bytes"
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// The helper process is this test binary, re-executed. It stands in for cargo
// and go, which a test cannot make fail on demand.
func TestMain(m *testing.M) {
	if len(os.Args) > 1 && strings.HasPrefix(os.Args[1], helperFlag) {
		switch strings.TrimPrefix(os.Args[1], helperFlag) {
		case "cargo-fail":
			fmt.Fprintln(os.Stderr, "   Running tests/x.rs (t/x-1)")
			fmt.Fprintln(os.Stdout, "test boom ... FAILED")
			fmt.Fprintln(os.Stdout, "test result: FAILED. 0 passed; 1 failed")
			fmt.Fprintln(os.Stderr, "error: test failed, to rerun pass `-p cb --test it`")
			os.Exit(101)
		case "go-json":
			fmt.Println(`{"Action":"output","Package":"p","Test":"TestA","Output":"--- FAIL: TestA\n"}`)
			fmt.Println(`{"Action":"fail","Package":"p","Test":"TestA"}`)
			fmt.Println(`{"Action":"fail","Package":"p"}`)
			os.Exit(1)
		case "hang":
			time.Sleep(time.Minute)
		case "scan":
			// A stand-in for a libtest binary: with a filter (a `--`) it ran
			// nothing, without one it ran two tests.
			for _, a := range os.Args[2:] {
				if a == "--" {
					fmt.Println("running 0 tests")
					os.Exit(0)
				}
			}
			fmt.Println("running 2 tests")
		case "ok":
			fmt.Println("fine")
		}
		os.Exit(0)
	}
	os.Exit(m.Run())
}

const helperFlag = "--dctestselect-helper="

func helper(mode string) Command {
	return Command{Display: "helper " + mode, Dir: ".", Argv: []string{os.Args[0], helperFlag + mode}}
}

func TestRunAllRunsEveryCommandAndReportsTheWorstOutcome(t *testing.T) {
	ok := helper("ok")
	var out, errs bytes.Buffer
	// The failing command comes first: the passing one after it must still run.
	failing := helper("cargo-fail")
	code := runAll(context.Background(), t.TempDir(), []Command{failing, ok}, time.Minute, "", &out, &errs)
	if code != 1 {
		t.Fatalf("exit = %d, want 1 for a failing test command", code)
	}
	if !strings.Contains(out.String(), "::error::helper cargo-fail exited 1") {
		t.Errorf("stdout = %q", out.String())
	}
	if strings.Count(out.String(), "::group::") != 2 {
		t.Errorf("a failure stopped the later commands: %q", out.String())
	}
}

func TestACommandThatCannotStartOrIsStoppedExitsTwoNotOne(t *testing.T) {
	var out, errs bytes.Buffer
	missing := Command{Display: "missing", Dir: ".", Argv: []string{filepath.Join(t.TempDir(), "no-such-binary")}}
	if code := runAll(context.Background(), t.TempDir(), []Command{missing}, time.Minute, "", &out, &errs); code != 2 {
		t.Errorf("a command that could not start exited %d, want 2", code)
	}
	started := time.Now()
	if code := runAll(context.Background(), t.TempDir(), []Command{helper("hang")}, 300*time.Millisecond, "", &out, &errs); code != 2 {
		t.Errorf("a command stopped at its deadline exited %d, want 2", code)
	}
	if time.Since(started) > 20*time.Second {
		t.Errorf("the deadline was not enforced: %s", time.Since(started))
	}
}

func TestTheFullPhaseKeepsCargosInterleavedOutputInOrderForTheAudit(t *testing.T) {
	dir := t.TempDir()
	c := helper("cargo-fail")
	c.Log = "full-rust.log"
	var out, errs bytes.Buffer
	if code := runAll(context.Background(), t.TempDir(), []Command{c}, time.Minute, dir, &out, &errs); code != 1 {
		t.Fatalf("exit = %d", code)
	}
	log, err := os.ReadFile(filepath.Join(dir, "full-rust.log"))
	if err != nil {
		t.Fatal(err)
	}
	want := "   Running tests/x.rs (t/x-1)\ntest boom ... FAILED\ntest result: FAILED. 0 passed; 1 failed\nerror: test failed, to rerun pass `-p cb --test it`\n"
	if string(log) != want {
		t.Fatalf("log = %q\nwant %q (stderr and stdout interleaved, as a person read them)", log, want)
	}
	// And the audit attributes the failure from exactly that log.
	got := parseLibtest(bytes.NewReader(log))
	if len(got.Failures) != 1 || got.Failures[0].Key() != "rust:cb:--test it:boom" {
		t.Errorf("failures = %v", keys(got.Failures))
	}
	if !strings.Contains(out.String(), "test boom ... FAILED") {
		t.Errorf("the output was not also shown in the job log: %q", out.String())
	}
}

func TestTheGoPhaseKeepsTheJSONAndShowsTheTestOutput(t *testing.T) {
	dir := t.TempDir()
	c := helper("go-json")
	c.GoJSON, c.Log = true, "full-go.jsonl"
	var out, errs bytes.Buffer
	if code := runAll(context.Background(), t.TempDir(), []Command{c}, time.Minute, dir, &out, &errs); code != 1 {
		t.Fatalf("exit = %d", code)
	}
	log, err := os.ReadFile(filepath.Join(dir, "full-go.jsonl"))
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(log), `"Action":"fail","Package":"p","Test":"TestA"`) {
		t.Errorf("the raw event stream was not kept: %q", log)
	}
	if !strings.Contains(out.String(), "--- FAIL: TestA") || strings.Contains(out.String(), `"Action"`) {
		t.Errorf("the job log must show what go test printed, not the JSON: %q", out.String())
	}
	if got := parseGoJSON(bytes.NewReader(log)); !got.Examined || len(got.Failures) != 1 || got.Failures[0].Key() != "go:p:TestA" {
		t.Errorf("parse of the kept log: %+v", got)
	}
}

func TestALogNameThatIsAPathIsRefused(t *testing.T) {
	c := helper("ok")
	c.Log = "../escape.log"
	var out, errs bytes.Buffer
	if code := runAll(context.Background(), t.TempDir(), []Command{c}, time.Minute, t.TempDir(), &out, &errs); code != 2 {
		t.Errorf("exit = %d, want 2", code)
	}
}

func TestRunPhaseSelectedSkipsWhenThePlanIsEverythingAndRunsTheSelectionOtherwise(t *testing.T) {
	dir := t.TempDir()
	skip := filepath.Join(dir, "everything.json")
	if err := writeJSON(skip, Plan{Schema: planSchema, Everything: true, Selected: []Selected{}, Full: []Command{}}); err != nil {
		t.Fatal(err)
	}
	var out bytes.Buffer
	if code := run(context.Background(), []string{"run", "--plan", skip, "--phase", "selected", "--repo", dir, "--log-dir", dir}, &out, &bytes.Buffer{}); code != 0 || !strings.Contains(out.String(), "fast phase is skipped") {
		t.Errorf("everything plan: code=%d out=%q", code, out.String())
	}
	some := filepath.Join(dir, "some.json")
	sel := Selected{Command: helper("ok"), Units: []Unit{{Kind: "go", Pkg: "p"}}}
	if err := writeJSON(some, Plan{Schema: planSchema, Selected: []Selected{sel}, Full: []Command{}}); err != nil {
		t.Fatal(err)
	}
	out.Reset()
	if code := run(context.Background(), []string{"run", "--plan", some, "--phase", "selected", "--repo", dir, "--log-dir", dir}, &out, &bytes.Buffer{}); code != 0 || !strings.Contains(out.String(), "fine") {
		t.Errorf("selection: code=%d out=%q", code, out.String())
	}
	if code := run(context.Background(), []string{"run", "--plan", some, "--phase", "bogus"}, &out, &bytes.Buffer{}); code != 2 {
		t.Errorf("a bad phase exited %d, want 2", code)
	}
}

func TestAPlanOfTheWrongSchemaOrWithUnknownFieldsIsRefused(t *testing.T) {
	dir := t.TempDir()
	for name, body := range map[string]string{
		"schema":  `{"schema":99}`,
		"unknown": `{"schema":1,"surprise":true}`,
		"trailer": `{"schema":1} {"schema":1}`,
	} {
		p := filepath.Join(dir, name+".json")
		if err := os.WriteFile(p, []byte(body), 0o644); err != nil {
			t.Fatal(err)
		}
		if _, err := readPlan(p); err == nil {
			t.Errorf("%s: accepted", name)
		}
	}
}

func TestAnnotationsEscapeWhatTheRunnerWouldReadAsTheEndOfTheMessage(t *testing.T) {
	if got := annotate("error", "a\nb%c\r"); got != "::error::a%0Ab%25c%0D" {
		t.Errorf("annotate = %q", got)
	}
}

func TestAFilteredRunThatRanNoTestIsRerunWhole(t *testing.T) {
	dir := t.TempDir()
	filtered := Command{Display: "filtered", Dir: ".", Log: "selected-rust.log", Argv: append(helper("scan").Argv, "--", "x::"), Fallback: helper("scan").Argv}
	var out, errs bytes.Buffer
	code, statuses := runPhase(context.Background(), t.TempDir(), []Command{filtered}, time.Minute, dir, &out, &errs)
	if code != 0 || len(statuses) != 2 {
		t.Fatalf("code=%d statuses=%d, want 0 and the filtered run plus the whole one\n%s", code, len(statuses), out.String())
	}
	if !strings.Contains(out.String(), "::notice::filter matched nothing: filtered") {
		t.Errorf("the re-run is not logged: %q", out.String())
	}
	log, _ := os.ReadFile(filepath.Join(dir, "selected-rust.log"))
	if !strings.Contains(string(log), "running 0 tests") || !strings.Contains(string(log), "running 2 tests") {
		t.Errorf("the log must hold both runs, appended: %q", log)
	}
	// A filtered run that ran tests is not repeated.
	ran := Command{Display: "ran", Dir: ".", Log: "selected-rust.log", Argv: helper("scan").Argv, Fallback: helper("scan").Argv}
	out.Reset()
	if _, statuses := runPhase(context.Background(), t.TempDir(), []Command{ran}, time.Minute, dir, &out, &errs); len(statuses) != 1 {
		t.Errorf("a run that ran tests was repeated: %d statuses", len(statuses))
	}
}

func TestRanNothingReadsLibtestsCountLines(t *testing.T) {
	for out, want := range map[string]bool{
		"":                                   true,
		"running 0 tests\n":                  true,
		"running 0 tests\nrunning 0 tests\n": true,
		"running 1 test\n":                   false,
		"running 0 tests\nrunning 3 tests\n": false,
		"\x1b[1mrunning 2 tests\x1b[0m\n":    false,
		"cargo said nothing useful\n":        true,
	} {
		if got := ranNothing(out); got != want {
			t.Errorf("ranNothing(%q) = %t, want %t", out, got, want)
		}
	}
}

func TestEveryCommandOfAPhaseLeavesAStatus(t *testing.T) {
	dir := t.TempDir()
	missing := Command{Display: "missing", Dir: ".", Log: "a.log", Argv: []string{filepath.Join(dir, "no-such-binary")}}
	failing := helper("cargo-fail")
	failing.Log = "b.log"
	good := helper("ok")
	good.Log = "c.log"
	planPath := filepath.Join(dir, "plan.json")
	if err := writeJSON(planPath, Plan{Schema: planSchema, Selected: []Selected{}, Full: []Command{missing, failing, good}}); err != nil {
		t.Fatal(err)
	}
	var out bytes.Buffer
	code := run(context.Background(), []string{"run", "--plan", planPath, "--phase", "full", "--repo", dir, "--log-dir", dir}, &out, &bytes.Buffer{})
	if code != 2 {
		t.Fatalf("exit = %d, want 2 for a command that could not start", code)
	}
	st, err := readStatus(filepath.Join(dir, "full-status.json"))
	if err != nil {
		t.Fatal(err)
	}
	if a := st["a.log"]; a == nil || a.Completed || a.Code != -1 || a.Detail == "" {
		t.Errorf("a command that never started: %+v", a)
	}
	if b := st["b.log"]; b == nil || !b.Completed || b.Code != 101 {
		t.Errorf("a command that exited 101: %+v", b)
	}
	if c := st["c.log"]; c == nil || !c.Completed || c.Code != 0 {
		t.Errorf("a command that passed: %+v", c)
	}
}

func TestAnUnreadableStatusFileIsAnError(t *testing.T) {
	dir := t.TempDir()
	for name, body := range map[string]string{"junk": "x", "schema": `{"schema":9,"commands":[]}`, "unknown": `{"schema":1,"commands":[],"x":1}`} {
		p := filepath.Join(dir, name)
		if err := os.WriteFile(p, []byte(body), 0o644); err != nil {
			t.Fatal(err)
		}
		if _, err := readStatus(p); err == nil {
			t.Errorf("%s: accepted", name)
		}
	}
	if _, err := readStatus(filepath.Join(dir, "absent")); err == nil {
		t.Error("an absent status file was accepted")
	}
}

func TestAMissingPlanRunsTheBuiltInFullSuiteAndSaysSoButNotTheFastPhase(t *testing.T) {
	dir := t.TempDir() // a repository with no rust/ or go directory: every command fails to start
	var out bytes.Buffer
	code := run(context.Background(), []string{"run", "--plan", filepath.Join(dir, "absent.json"), "--phase", "full", "--repo", dir, "--log-dir", dir}, &out, &bytes.Buffer{})
	if code != 2 {
		t.Fatalf("exit = %d; the built-in suite could not run here, which is a failure, not a pass", code)
	}
	if !strings.Contains(out.String(), "::error::the plan is unusable") || !strings.Contains(out.String(), "cargo test --locked --workspace --exclude dc-glob") || !strings.Contains(out.String(), "go test ./... -count=1 -json") {
		t.Errorf("output = %q; want the error and both built-in commands attempted", out.String())
	}
	body, err := os.ReadFile(filepath.Join(dir, "full-status.json"))
	if err != nil || strings.Count(string(body), `"cmd"`) != 2 {
		t.Errorf("status file = %q err=%v", body, err)
	}
	out.Reset()
	if code := run(context.Background(), []string{"run", "--plan", filepath.Join(dir, "absent.json"), "--phase", "selected", "--repo", dir, "--log-dir", dir}, &out, &bytes.Buffer{}); code != 2 || strings.Contains(out.String(), "cargo") {
		t.Errorf("the selected phase with no plan: exit=%d out=%q", code, out.String())
	}
}

func TestCargoCommandsRunWithoutColour(t *testing.T) {
	p := BuildPlan(inputs(withTests("rust/ca/src/engine.rs", "go/pkga/a_test.go")))
	all := append([]Command{}, p.Full...)
	for _, s := range p.Selected {
		all = append(all, s.Command)
	}
	cargo := 0
	for _, c := range all {
		if c.Argv[0] != "cargo" {
			continue
		}
		cargo++
		if len(c.Env) != 1 || c.Env[0] != "CARGO_TERM_COLOR=never" {
			t.Errorf("%q env = %v; ANSI escapes hide lines from the audit", c.Display, c.Env)
		}
	}
	if cargo < 3 {
		t.Errorf("only %d cargo commands checked", cargo)
	}
}

func TestWorkflowCommandValuesAreEscaped(t *testing.T) {
	if got := annotate("error", "a\nb%c\r"); got != "::error::a%0Ab%25c%0D" {
		t.Errorf("annotate = %q", got)
	}
	if got := escapeProperty("k:v,w%\n"); got != "k%3Av%2Cw%25%0A" {
		t.Errorf("escapeProperty = %q", got)
	}
	if got := escapeData("a:b,c"); got != "a:b,c" {
		t.Errorf("escapeData must leave : and , alone in a message: %q", got)
	}
	if got := group("x\n::error::y"); strings.Contains(got, "\n") || got != "::group::x%0A::error::y" {
		t.Errorf("group = %q", got)
	}
}
