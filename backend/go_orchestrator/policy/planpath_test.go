package policy

import (
	"encoding/json"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
)

// A plan entry spelled "./src/a.go" is the entry "src/a.go" on every reader:
// the write gate, PlansPath and the planned-path judge. Before the one strip
// rule, the gate compared the pattern verbatim and a ./ entry authorised nothing
// (and a ./ read_only entry restricted nothing).
func TestDotSlashPlanEntryIsOneEntryOnEveryReader(t *testing.T) {
	var planned []dc.PlannedFile
	if err := json.Unmarshal([]byte(`[{"path":"./src/a.go","allowed_change":"read_only"},{"path":"src/b.go","allowed_change":"modify"}]`), &planned); err != nil {
		t.Fatal(err)
	}
	for name, files := range map[string][]dc.PlannedFile{
		"decoded": planned,
		"as-typed": {
			{Path: "./src/a.go", AllowedChange: dc.ChangeReadOnly},
			{Path: "src/b.go", AllowedChange: dc.ChangeModify},
		},
	} {
		t.Run(name, func(t *testing.T) {
			task := &dc.Task{ID: "TASK-1", PlannedFiles: files}
			g := mapless(t)
			if d := decide(t, g, "src/a.go", task, dc.OpModify); d.Rule != RuleReadOnly || !d.Blocked() {
				t.Errorf("write to ./src/a.go read_only: rule=%s blocked=%v, want %s", d.Rule, d.Blocked(), RuleReadOnly)
			}
			if d := decide(t, g, "src/b.go", task, dc.OpModify); d.Blocked() {
				t.Errorf("write to planned src/b.go refused: %+v", d)
			}
			if !PlansPath(task, "src/a.go") || !PlansPath(task, "./src/a.go") {
				t.Error("PlansPath must answer true for planned ./src/a.go")
			}
			if !MatchesPlannedPath("src/a.go", files) || !MatchesPlannedPath("./src/a.go", files) {
				t.Error("MatchesPlannedPath must answer true for planned ./src/a.go")
			}
		})
	}
}

func TestPlansPathReadsGlobEntries(t *testing.T) {
	task := &dc.Task{ID: "TASK-1", PlannedFiles: []dc.PlannedFile{modify("docs/*.md")}}
	if !PlansPath(task, "docs/x.md") {
		t.Error("a glob entry plans the paths it matches, as verification's orphan check reads it")
	}
	if PlansPath(task, "src/x.md") {
		t.Error("a glob entry must not plan a path it does not match")
	}
}

// A glob entry is not a licence over what the hard rules protect: the gate
// refuses those paths before it reads the plan, so verification and get_diff must
// not read a "*" plan as covering them. An entry naming the path exactly is read
// as it always was.
func TestGlobEntryNeverCoversAHardRulePath(t *testing.T) {
	glob := []dc.PlannedFile{modify("*"), modify("./**")}
	for _, path := range []string{
		".env", ".git/config", ".devcouncil/config.yaml", ".devmap/repo_map.json", "deploy/server.pem",
		// prompt inputs: the gate refuses these before it reads the plan
		"CLAUDE.md", "AGENTS.md", "plugins/x/mcp.json", ".mcp.json", ".claude/skills/x/SKILL.md",
		// malformed or aliasing spellings, which the gate refuses raw and cleaned
		"a.", ".git /config", "a\nb.go",
	} {
		if MatchesPlannedPath(path, glob) {
			t.Errorf("a glob entry covers hard-rule path %s", path)
		}
		if editable, _ := PlannedScope(path, glob); editable {
			t.Errorf("a glob entry makes %s editable", path)
		}
		// An entry naming the path exactly is read as it always was; the gate
		// refuses the write regardless.
		if !MatchesPlannedPath(path, []dc.PlannedFile{modify("./" + path)}) {
			t.Errorf("an entry naming %s exactly must still match it", path)
		}
	}
	if !MatchesPlannedPath("src/a.go", glob) {
		t.Error("a glob entry must still cover an ordinary path")
	}
}

func TestPlannedScopeClassifiesByEntryKind(t *testing.T) {
	planned := []dc.PlannedFile{
		{Path: "./src/*.go", AllowedChange: dc.ChangeModify},
		{Path: "src/a.go", AllowedChange: dc.ChangeReadOnly},
	}
	if editable, ro := PlannedScope("src/a.go", planned); !editable || !ro {
		t.Errorf("src/a.go: editable=%v readOnly=%v, want both (overlap)", editable, ro)
	}
	if editable, ro := PlannedScope("src/b.go", planned); !editable || ro {
		t.Errorf("src/b.go: editable=%v readOnly=%v, want editable only", editable, ro)
	}
	if editable, ro := PlannedScope("docs/x.md", planned); editable || ro {
		t.Errorf("docs/x.md: editable=%v readOnly=%v, want neither", editable, ro)
	}
}

// The invariant behind the rows above: wherever the write gate refuses a path by
// a hard rule before it reads the plan, no glob entry plans it, in the judge
// verify and get_diff use. HardDenied is the gate's own pre-task function, so
// this also pins that the gate and the judge cannot drift.
func TestNoGlobPlansAPathTheGateHardDenies(t *testing.T) {
	glob := []dc.PlannedFile{modify("*"), modify("./**")}
	task := &dc.Task{ID: "TASK-1", PlannedFiles: glob}
	g := FileGate{Root: t.TempDir(), HardRules: true}
	hard := map[RuleID]bool{RuleSecretPath: true, RuleRestrictedPath: true, RuleMalformedPath: true, RuleOutsideRoot: true}
	denied := 0
	for _, path := range []string{
		".env", ".env.local", "a/.env", "secrets/k", "deploy/server.pem", "id_rsa", ".npmrc",
		".git/config", ".GIT/config", ".git", ".claude/settings.json", ".codex/x", ".devcouncil/state.sqlite", ".devmap/g.sqlite",
		"CLAUDE.md", "AGENTS.md", "docs/AGENTS.md", "mcp.json", ".mcp.json", "plugins/x/mcp.json", ".claude/skills/x/SKILL.md",
		"a.", "a ", ".git /config", "a\nb.go", "a\x00b", "src/../.env", ".//.env", "../x",
	} {
		d := g.EvaluateFileChange(path, task, dc.OpModify, false)
		if !d.Blocked() || !hard[d.Rule] {
			continue
		}
		denied++
		if editable, _ := PlannedScope(path, glob); editable {
			t.Errorf("%q: gate hard-denies (%s) but a glob plans it", path, d.Rule)
		}
		if !HardDenied("", path) {
			t.Errorf("%q: gate hard-denies (%s) but HardDenied with no root does not", path, d.Rule)
		}
	}
	if denied < 20 {
		t.Fatalf("only %d paths were hard denials; the matrix no longer exercises the invariant", denied)
	}
}

// read_only wins at the gate whatever the order or spelling of the overlapping
// entries, as it does in verify.
func TestGateReadOnlyWinsOverlapRegardlessOfOrder(t *testing.T) {
	ro := func(p string) dc.PlannedFile { return dc.PlannedFile{Path: p, AllowedChange: dc.ChangeReadOnly} }
	for _, pair := range [][2]dc.PlannedFile{
		{modify("src/a.go"), ro("src/a.go")},
		{modify("src/a.go"), ro("src/*.go")},
		{modify("src/a.go"), ro("./src/a.go")},
		{modify("src/*"), ro("src/a.go")},
	} {
		for _, files := range [][]dc.PlannedFile{{pair[0], pair[1]}, {pair[1], pair[0]}} {
			if d := decide(t, mapless(t), "src/a.go", planTask(files...), dc.OpModify); d.Rule != RuleReadOnly {
				t.Errorf("%+v: rule=%s, want %s", files, d.Rule, RuleReadOnly)
			}
		}
	}
	// The overlap restricts only what the read_only entry names.
	if d := decide(t, mapless(t), "src/b.go", planTask(modify("src/*"), ro("src/a.go")), dc.OpModify); d.Blocked() {
		t.Errorf("src/b.go is planned and not read_only: %+v", d)
	}
}

func TestForbiddenEntriesAreNormalisedByTheGate(t *testing.T) {
	task := &dc.Task{ID: "TASK-1", PlannedFiles: []dc.PlannedFile{modify("*")}, ForbiddenChanges: []string{"./secret/*"}}
	if d := decide(t, mapless(t), "secret/x", task, dc.OpModify); d.Rule != RuleForbiddenChange {
		t.Errorf("rule=%s, want %s", d.Rule, RuleForbiddenChange)
	}
}
