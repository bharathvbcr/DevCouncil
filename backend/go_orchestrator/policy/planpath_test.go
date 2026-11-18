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
// refuses those paths before it reads the plan, and verification must not read a
// "*" plan as covering them. An entry naming the path exactly is unchanged.
func TestGlobEntryNeverCoversAHardRulePath(t *testing.T) {
	glob := []dc.PlannedFile{modify("*"), modify("./**")}
	for _, path := range []string{".env", ".git/config", ".devcouncil/config.yaml", ".devmap/repo_map.json", "deploy/server.pem"} {
		if MatchesPlannedPath(path, glob) {
			t.Errorf("a glob entry covers hard-rule path %s", path)
		}
		if editable, _ := PlannedScope(path, glob); editable {
			t.Errorf("a glob entry makes %s editable", path)
		}
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
