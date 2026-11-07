package main

import (
	"bytes"
	"context"
	"errors"
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
			fmt.Fprintln(os.Stderr, "     Running tests/x.rs (t/x-1)")
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
		case "no-status":
			// Fails if a status file from an earlier run is still there.
			if _, err := os.Stat("full-status.json"); err == nil {
				fmt.Println("a stale full-status.json was still present")
				os.Exit(3)
			}
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
	want := "     Running tests/x.rs (t/x-1)\ntest boom ... FAILED\ntest result: FAILED. 0 passed; 1 failed\nerror: test failed, to rerun pass `-p cb --test it`\n"
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
	if err := writeAtomic(skip, Plan{Schema: planSchema, Everything: true, Selected: []Selected{}, Full: []Command{helper("ok")}}); err != nil {
		t.Fatal(err)
	}
	var out bytes.Buffer
	if code := run(context.Background(), []string{"run", "--plan", skip, "--phase", "selected", "--repo", dir, "--log-dir", dir}, &out, &bytes.Buffer{}); code != 0 || !strings.Contains(out.String(), "fast phase is skipped") {
		t.Errorf("everything plan: code=%d out=%q", code, out.String())
	}
	some := filepath.Join(dir, "some.json")
	sel := Selected{Command: helper("ok"), Units: []Unit{{Kind: "go", Pkg: "p"}}}
	if err := writeAtomic(some, Plan{Schema: planSchema, Selected: []Selected{sel}, Full: []Command{helper("ok")}}); err != nil {
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
		if _, _, err := loadPlan(p); err == nil {
			t.Errorf("%s: accepted", name)
		}
	}
}

func TestAnnotationsEscapeWhatTheRunnerWouldReadAsTheEndOfTheMessage(t *testing.T) {
	if got := annotate("error", "a\nb%c\r"); got != "::error::a%0Ab%25c%0D" {
		t.Errorf("annotate = %q", got)
	}
}

func TestEveryCommandOfAPhaseLeavesAStatus(t *testing.T) {
	dir := t.TempDir()
	missing := Command{Display: "missing", Dir: ".", Log: "a.log", Argv: []string{filepath.Join(dir, "no-such-binary")}}
	failing := helper("cargo-fail")
	failing.Log = "b.log"
	good := helper("ok")
	good.Log = "c.log"
	useFullSuite(t, missing, failing, good)
	planPath := filepath.Join(dir, "plan.json")
	if err := writeAtomic(planPath, Plan{Schema: planSchema, Selected: []Selected{}, Full: []Command{missing, failing, good}}); err != nil {
		t.Fatal(err)
	}
	var out bytes.Buffer
	code := run(context.Background(), []string{"run", "--plan", planPath, "--phase", "full", "--repo", dir, "--log-dir", dir}, &out, &bytes.Buffer{})
	if code != 2 {
		t.Fatalf("exit = %d, want 2 for a command that could not start", code)
	}
	st, err := loadStatus(filepath.Join(dir, "full-status.json"), planSHA(t, planPath))
	if err != nil {
		t.Fatal(err)
	}
	byLog := st.byLog()
	if a := byLog["a.log"]; a == nil || a.Completed || a.Code != -1 || a.Detail == "" {
		t.Errorf("a command that never started: %+v", a)
	}
	if b := byLog["b.log"]; b == nil || !b.Completed || b.Code != 101 {
		t.Errorf("a command that exited 101: %+v", b)
	}
	if c := byLog["c.log"]; c == nil || !c.Completed || c.Code != 0 {
		t.Errorf("a command that passed: %+v", c)
	}
}

func TestAnUnreadableStatusFileIsAnError(t *testing.T) {
	dir := t.TempDir()
	for name, body := range map[string]string{
		"junk":       "x",
		"schema":     `{"schema":9,"plan_sha256":"p","commands":[]}`,
		"unknown":    `{"schema":2,"plan_sha256":"p","commands":[],"x":1}`,
		"other plan": `{"schema":2,"plan_sha256":"q","commands":[]}`,
	} {
		p := filepath.Join(dir, name)
		if err := os.WriteFile(p, []byte(body), 0o644); err != nil {
			t.Fatal(err)
		}
		if _, err := loadStatus(p, "p"); err == nil {
			t.Errorf("%s: accepted", name)
		}
	}
	if _, err := loadStatus(filepath.Join(dir, "absent"), "p"); err == nil {
		t.Error("an absent status file was accepted")
	}
}

func TestAPhaseRemovesTheStaleStatusAtStartAndWritesItsOwnWhole(t *testing.T) {
	dir := t.TempDir()
	stale := filepath.Join(dir, "full-status.json")
	if err := os.WriteFile(stale, []byte(`{"stale":true}`), 0o644); err != nil {
		t.Fatal(err)
	}
	c := helper("no-status")
	c.Log = "c.log"
	useFullSuite(t, c)
	planPath := filepath.Join(dir, "plan.json")
	if err := writeAtomic(planPath, Plan{Schema: planSchema, Selected: []Selected{}, Full: []Command{c}}); err != nil {
		t.Fatal(err)
	}
	var out bytes.Buffer
	// --repo dir makes the command's working directory the log directory.
	if code := run(context.Background(), []string{"run", "--plan", planPath, "--phase", "full", "--repo", dir, "--log-dir", dir}, &out, &bytes.Buffer{}); code != 0 {
		t.Fatalf("exit = %d: a stale full-status.json was still there while the phase ran\n%s", code, out.String())
	}
	st, err := loadStatus(stale, planSHA(t, planPath))
	if err != nil || len(st.Commands) != 1 || !st.Commands[0].Completed {
		t.Fatalf("status = %+v err=%v", st, err)
	}
	if left, _ := filepath.Glob(filepath.Join(dir, "*.tmp*")); len(left) != 0 {
		t.Errorf("temp files left behind: %v", left)
	}
}

func failRename(t *testing.T) {
	t.Helper()
	prev := renameFile
	renameFile = func(string, string) error { return errors.New("injected: the rename failed") }
	t.Cleanup(func() { renameFile = prev })
}

// A write that fails after the temp file is complete but before it is
// published must leave the old document whole and no temp file behind. A plain
// os.WriteFile has no such step, so it replaces the content and fails this.
func TestWriteAtomicLeavesTheOldDocumentWholeWhenPublishingFails(t *testing.T) {
	dir := t.TempDir()
	p := filepath.Join(dir, "s.json")
	if err := writeAtomic(p, map[string]int{"a": 1}); err != nil {
		t.Fatal(err)
	}
	old, _ := os.ReadFile(p)
	failRename(t)
	if err := writeAtomic(p, map[string]int{"b": 2}); err == nil {
		t.Fatal("a failed rename was reported as success")
	}
	if body, _ := os.ReadFile(p); string(body) != string(old) {
		t.Errorf("body = %s, want the old document %s", body, old)
	}
	if left, _ := filepath.Glob(filepath.Join(dir, "*.tmp*")); len(left) != 0 {
		t.Errorf("temp files left behind: %v", left)
	}
	if err := writeAtomic(filepath.Join(dir, "no", "such", "s.json"), 1); err == nil {
		t.Error("a write into a missing directory succeeded")
	}
}

func TestWriteAtomicReplacesTheDocument(t *testing.T) {
	p := filepath.Join(t.TempDir(), "s.json")
	for _, v := range []map[string]int{{"a": 1}, {"b": 2}} {
		if err := writeAtomic(p, v); err != nil {
			t.Fatal(err)
		}
	}
	if body, _ := os.ReadFile(p); !strings.Contains(string(body), `"b": 2`) || strings.Contains(string(body), `"a"`) {
		t.Errorf("body = %s", body)
	}
}

// plan.json is published the same way: a Plan step killed or failing mid-write
// must not leave a truncated plan over the last whole one.
func TestThePlanIsPublishedAtomically(t *testing.T) {
	dir := t.TempDir()
	out := filepath.Join(dir, "plan.json")
	if err := os.WriteFile(out, []byte("old plan\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	failRename(t)
	var stdout bytes.Buffer
	code := run(context.Background(), []string{"plan", "--repo", dir, "--devmap", filepath.Join(dir, "no-devmap"), "--timeout", "10s", "--out", out}, &stdout, &bytes.Buffer{})
	if code != 2 {
		t.Errorf("exit = %d, want 2 when the plan could not be published", code)
	}
	if body, _ := os.ReadFile(out); string(body) != "old plan\n" {
		t.Errorf("plan.json = %q; a failed publish replaced the old plan", body)
	}
	if left, _ := filepath.Glob(filepath.Join(dir, "*.tmp*")); len(left) != 0 {
		t.Errorf("temp files left behind: %v", left)
	}
}

func TestAPlanWithNoFullCommandsRunsTheBuiltInSuiteAndExitsNonZero(t *testing.T) {
	dir := t.TempDir() // no rust/ or go directory: every built-in command fails to start
	planPath := filepath.Join(dir, "plan.json")
	if err := os.WriteFile(planPath, []byte(`{"schema":1}`), 0o644); err != nil {
		t.Fatal(err)
	}
	var out bytes.Buffer
	code := run(context.Background(), []string{"run", "--repo", dir, "--plan", planPath, "--phase", "full", "--log-dir", dir}, &out, &bytes.Buffer{})
	if code == 0 {
		t.Fatalf("run --phase full exited 0 having run nothing\n%s", out.String())
	}
	if !strings.Contains(out.String(), "::error::the plan is unusable") || !strings.Contains(out.String(), "cargo test --locked --workspace") {
		t.Errorf("the built-in suite was not attempted, or the plan not called unusable:\n%s", out.String())
	}
	st, err := loadStatus(filepath.Join(dir, "full-status.json"), builtinPlan)
	if err != nil || len(st.Commands) != 2 {
		t.Errorf("status = %+v err=%v", st, err)
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
	p := BuildPlan(inputs(withTests("rust/ca/src/engine.rs", "rust/cb/src/lib.rs", "go/pkga/a_test.go")))
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
	if cargo < 2 {
		t.Errorf("only %d cargo commands checked", cargo)
	}
}

func TestWorkflowCommandValuesAreEscaped(t *testing.T) {
	if got := annotate("error", "a\nb%c\r"); got != "::error::a%0Ab%25c%0D" {
		t.Errorf("annotate = %q", got)
	}
	if got := escapeData("a:b,c"); got != "a:b,c" {
		t.Errorf("escapeData must leave : and , alone in a message: %q", got)
	}
	if got := group("x\n::error::y"); strings.Contains(got, "\n") || got != "::group::x%0A::error::y" {
		t.Errorf("group = %q", got)
	}
}
