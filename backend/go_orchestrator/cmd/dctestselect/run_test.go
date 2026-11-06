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
			fmt.Fprintln(os.Stderr, "error: test failed, to rerun pass `-p cb --test it`")
			os.Exit(101)
		case "go-json":
			fmt.Println(`{"Action":"output","Package":"p","Test":"TestA","Output":"--- FAIL: TestA\n"}`)
			fmt.Println(`{"Action":"fail","Package":"p","Test":"TestA"}`)
			fmt.Println(`{"Action":"fail","Package":"p"}`)
			os.Exit(1)
		case "hang":
			time.Sleep(time.Minute)
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
	want := "   Running tests/x.rs (t/x-1)\ntest boom ... FAILED\nerror: test failed, to rerun pass `-p cb --test it`\n"
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
	if code := run(context.Background(), []string{"run", "--plan", skip, "--phase", "selected", "--repo", dir}, &out, &bytes.Buffer{}); code != 0 || !strings.Contains(out.String(), "fast phase is skipped") {
		t.Errorf("everything plan: code=%d out=%q", code, out.String())
	}
	some := filepath.Join(dir, "some.json")
	sel := Selected{Command: helper("ok"), Units: []Unit{{Kind: "go", Pkg: "p"}}}
	if err := writeJSON(some, Plan{Schema: planSchema, Selected: []Selected{sel}, Full: []Command{}}); err != nil {
		t.Fatal(err)
	}
	out.Reset()
	if code := run(context.Background(), []string{"run", "--plan", some, "--phase", "selected", "--repo", dir}, &out, &bytes.Buffer{}); code != 0 || !strings.Contains(out.String(), "fine") {
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
