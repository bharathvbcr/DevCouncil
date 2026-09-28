package policy

import (
	"errors"
	"reflect"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
)

type failingMatcher struct{ calls int }

func (m *failingMatcher) MatchAny([]string, string) (bool, error) {
	m.calls++
	return false, errors.New("engine is down")
}

func (m *failingMatcher) MatchAnyFold([]string, string) (bool, error) {
	m.calls++
	return false, errors.New("engine is down")
}

// countingMatcher is GoMatcher, counted: the decisions must be the ones
// GoMatcher gives, and it proves the gate asked through the interface.
type countingMatcher struct{ calls int }

func (m *countingMatcher) MatchAny(p []string, n string) (bool, error) {
	m.calls++
	return GoMatcher.MatchAny(p, n)
}

func (m *countingMatcher) MatchAnyFold(p []string, n string) (bool, error) {
	m.calls++
	return GoMatcher.MatchAnyFold(p, n)
}

func engineTask() *dc.Task {
	return &dc.Task{
		ID:              "TASK-001",
		PlannedFiles:    []dc.PlannedFile{{Path: "src/*.go"}},
		AllowedCommands: []string{"go test *"},
	}
}

// A matcher that cannot answer denies, under its own rule, on every entry
// point — including a path every rung would have allowed. A secret-path or
// allowlist rule here would record something false about the input.
func TestFailingMatcherDeniesUnderTheEngineRule(t *testing.T) {
	root := t.TempDir()
	task := engineTask()
	m := &failingMatcher{}
	fg := FileGate{Root: root, HardRules: true, Matcher: m}
	cg := CommandGate{Root: root, HardRules: true, Matcher: m}

	for name, d := range map[string]Decision{
		"write":            fg.EvaluateFileChange("src/a.go", task, dc.OpModify, false),
		"read":             fg.EvaluateRead("src/a.go", task),
		"write, no task":   fg.EvaluateFileChange("src/a.go", nil, dc.OpModify, false),
		"write, soft only": FileGate{Root: root, Matcher: m}.EvaluateFileChange("src/a.go", task, dc.OpModify, false),
	} {
		if d.Action != Deny || d.Rule != RulePathEngineUnavailable || d.Severity != Hard {
			t.Errorf("%s: %s/%s/%s, want deny/%s/hard (%s)", name, d.Action, d.Rule, d.Severity, RulePathEngineUnavailable, d.Reason)
		}
		// The record names what was being judged, not "".
		if d.Target != "src/a.go" {
			t.Errorf("%s: target %q, want src/a.go", name, d.Target)
		}
	}
	d := cg.EvaluateCommand("go test ./...", task)
	if d.Action != Deny || d.Rule != RuleCommandEngineUnavailable || d.Severity != Hard {
		t.Errorf("command: %s/%s/%s, want deny/%s/hard (%s)", d.Action, d.Rule, d.Severity, RuleCommandEngineUnavailable, d.Reason)
	}
	if d.Target != "go test ./..." || d.TaskID != "TASK-001" {
		t.Errorf("command: target %q task %q, want the command and TASK-001", d.Target, d.TaskID)
	}
	if w := fg.EvaluateFileChange("src/a.go", task, dc.OpModify, false); w.TaskID != "TASK-001" {
		t.Errorf("write: task %q, want TASK-001", w.TaskID)
	}
	if m.calls == 0 {
		t.Fatal("the gates never asked the matcher")
	}
	// Like every other decision, the engine denial notes a gate whose hard
	// rules were switched off, and stays Hard.
	soft := FileGate{Root: root, Matcher: m}.EvaluateFileChange("src/a.go", task, dc.OpModify, false)
	if soft.Severity != Hard || !reflect.DeepEqual(soft.Degraded, []string{"policy.hard_rules.disabled"}) {
		t.Errorf("hard rules off: severity %s degraded %v", soft.Severity, soft.Degraded)
	}
	if !IsCommandRule(RuleCommandEngineUnavailable) || IsCommandRule(RulePathEngineUnavailable) {
		t.Fatal("engine rules are filed under the wrong subject")
	}
}

// Rungs that decide before any pattern question are unaffected by a failing
// matcher: a malformed or outside-root path is refused for what it is.
func TestFailingMatcherDoesNotRelabelEarlierRungs(t *testing.T) {
	fg := FileGate{Root: t.TempDir(), HardRules: true, Matcher: &failingMatcher{}}
	if d := fg.EvaluateFileChange("../outside.go", engineTask(), dc.OpModify, false); d.Rule != RuleOutsideRoot {
		t.Fatalf("outside root: rule %s, want %s", d.Rule, RuleOutsideRoot)
	}
}

// A working matcher is asked, and gets exactly GoMatcher's decisions.
func TestInjectedMatcherIsAskedAndAgreesWithGo(t *testing.T) {
	root := t.TempDir()
	task := engineTask()
	m := &countingMatcher{}
	for _, path := range []string{"src/a.go", ".env", "src/sub/a.go", "README.md", ".git/config", "package.json"} {
		for _, op := range []dc.Operation{dc.OpModify, dc.OpCreate, dc.OpDelete} {
			want := FileGate{Root: root, HardRules: true}.EvaluateFileChange(path, task, op, false)
			got := FileGate{Root: root, HardRules: true, Matcher: m}.EvaluateFileChange(path, task, op, false)
			if !reflect.DeepEqual(got, want) {
				t.Errorf("%s %s: %+v, GoMatcher gives %+v", op, path, got, want)
			}
		}
	}
	for _, cmd := range []string{"go test ./...", "rm -rf /", "git status", "dev status", "echo hi > .env"} {
		want := CommandGate{Root: root, HardRules: true}.EvaluateCommand(cmd, task)
		got := CommandGate{Root: root, HardRules: true, Matcher: m}.EvaluateCommand(cmd, task)
		if !reflect.DeepEqual(got, want) {
			t.Errorf("%q: %+v, GoMatcher gives %+v", cmd, got, want)
		}
	}
	if m.calls == 0 {
		t.Fatal("the injected matcher was never asked")
	}
}

type panickingMatcher struct{}

func (panickingMatcher) MatchAny([]string, string) (bool, error)     { panic("not an engine failure") }
func (panickingMatcher) MatchAnyFold([]string, string) (bool, error) { panic("not an engine failure") }

// failClosed converts only engine failures; any other panic is a defect and
// must not be laundered into a tidy denial.
func TestNonEnginePanicIsNotSwallowed(t *testing.T) {
	defer func() {
		if r := recover(); r != "not an engine failure" {
			t.Fatalf("recovered %v, want the original panic", r)
		}
	}()
	FileGate{Root: t.TempDir(), HardRules: true, Matcher: panickingMatcher{}}.
		EvaluateFileChange("src/a.go", engineTask(), dc.OpModify, false)
	t.Fatal("the panic was swallowed")
}
