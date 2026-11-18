package devcouncil_test

import (
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/policy"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// scopeRow is one line of the fixture every scope surface is driven through:
// the plan entries a task carries, one path an agent changes, how, and whether
// that path is in scope. get_diff, policy_check_write and verify must all give
// that answer. Rows avoid what is not a scope question — read_only entries
// (get_diff lists a planned read-only file, the gate refuses it) and new test
// files (advisory in verify and the preflight, outside the plan for get_diff) —
// which have their own tests.
type scopeRow struct {
	name    string
	planned []string // modify entries
	path    string
	op      string // create | modify | delete
	inScope bool
	change  string // allowed_change of every entry; "" means modify
}

var scopeRows = []scopeRow{
	{"exact", []string{"src/a.go"}, "src/a.go", "modify", true, ""},
	{"same dir", []string{"src/a.go"}, "src/other.go", "modify", false, ""},
	{"same subsystem", []string{"src/a.go"}, "src/sub/deep.go", "modify", false, ""},
	{"root file", []string{"src/a.go"}, "README.md", "modify", false, ""},
	{"manifest", []string{"src/a.go"}, "go.mod", "modify", false, ""},
	{"planned manifest", []string{"go.mod"}, "go.mod", "modify", true, ""},
	{"delete planned for delete", []string{"src/a.go"}, "src/a.go", "delete", true, "delete"},
	{"dot-slash entry", []string{"./src/a.go"}, "src/a.go", "modify", true, ""},
	{"dot-slash entry, unplanned sibling", []string{"./src/a.go"}, "src/other.go", "modify", false, ""},
	{"glob entry", []string{"src/*.go"}, "src/a.go", "modify", true, ""},
	{"glob entry, outside", []string{"src/*.go"}, "docs/x.md", "modify", false, ""},
	{"dot-slash glob", []string{"./src/*.go"}, "src/a.go", "modify", true, ""},
	// A backslash entry cannot be seeded: dc-store refuses it (invalid_scope).
	// It reaches the detectors and the gate only through hand-written JSON, and
	// those readers are pinned in verify's planned_judge_test.go and policy's
	// planpath_test.go.
	{"case-variant entry", []string{"SRC/a.go"}, "src/a.go", "modify", false, ""},
}

func TestScopeSurfacesAgreeOnTheSharedFixtureTable(t *testing.T) {
	root := t.TempDir()
	git := func(args ...string) {
		t.Helper()
		cmd := exec.Command("git", append([]string{"-C", root, "-c", "user.name=t", "-c", "user.email=t@t"}, args...)...)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v %s", args, err, out)
		}
	}
	write := func(rel, body string) {
		t.Helper()
		full := filepath.Join(root, rel)
		if err := os.MkdirAll(filepath.Dir(full), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(full, []byte(body), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	git("init", "-q")
	for _, p := range []string{"src/a.go", "src/other.go", "src/sub/deep.go", "README.md", "go.mod", "docs/x.md"} {
		write(p, "v1\n")
	}
	git("add", "-A")
	git("commit", "-q", "-m", "seed")

	client := store.New(testsupport.DCStore(t), filepath.Join(t.TempDir(), "state.sqlite"))
	if _, err := client.ActiveLeases(t.Context()); err != nil {
		t.Fatal(err)
	}
	for i, row := range scopeRows {
		entries := make([]map[string]string, len(row.planned))
		for j, p := range row.planned {
			change := row.change
			if change == "" {
				change = "modify"
			}
			entries[j] = map[string]string{"path": p, "allowed_change": change}
		}
		scope, _ := json.Marshal(map[string]any{"planned_files": entries})
		testsupport.SeedTask(t, client.DB, testsupport.TaskSeed{ID: fmt.Sprintf("ROW-%d", i), Scope: string(scope)})
	}
	reg := devcouncil.NewRegistry(root, client, enforcingGate(t, root))

	for i, row := range scopeRows {
		t.Run(row.name, func(t *testing.T) {
			taskID := fmt.Sprintf("ROW-%d", i)
			if row.op == "delete" {
				if err := os.Remove(filepath.Join(root, row.path)); err != nil {
					t.Fatal(err)
				}
			} else {
				write(row.path, "v2 changed\n")
			}
			t.Cleanup(func() { write(row.path, "v1\n") })

			// get_diff: the path is in scope when the task-scoped diff names it.
			out, err := reg.Call(t.Context(), "devcouncil_get_diff", map[string]any{"task_id": taskID, "paths": []any{row.path}})
			if err != nil {
				t.Fatal(err)
			}
			diff, ok := out.(devcouncil.DiffResult)
			if !ok || !diff.OK {
				t.Fatalf("get_diff: got %T %+v", out, out)
			}
			diffIn := false
			for _, f := range diff.Files {
				diffIn = diffIn || f.Path == row.path
			}

			// policy_check_write: in scope is allowed with no would_block_verification.
			args := map[string]any{"path": row.path, "task_id": taskID, "operation": row.op}
			verdict := policyCheck(t, reg, args).(map[string]any)
			allowed, _ := verdict["allowed"].(bool)
			wouldBlock, _ := verdict["would_block_verification"].(bool)
			policyIn := allowed && !wouldBlock

			// verify: in scope is no blocking gap from the orphan detector.
			stored, err := client.Task(t.Context(), taskID)
			if err != nil || stored == nil {
				t.Fatalf("task %s: %v", taskID, err)
			}
			verifyIn := true
			for _, g := range verify.DetectOrphanDiffGaps(taskID, stored.PlannedFiles, []string{row.path}, nil) {
				verifyIn = verifyIn && !g.Blocking
			}

			if diffIn != row.inScope || policyIn != row.inScope || verifyIn != row.inScope {
				t.Errorf("%s %s planned %q: get_diff=%v policy_check_write=%v verify=%v, fixture says in-scope=%v",
					row.op, row.path, row.planned, diffIn, policyIn, verifyIn, row.inScope)
			}
		})
	}
}

// A planned modify entry plans the path for get_diff and verify, but the gate
// refuses a delete of it on the operation rung. Pinned so the shared table's
// omission of the delete row is a stated difference, not an untested one.
func TestDeleteOfAModifyPlannedPathIsAnOperationRefusalNotAScopeOne(t *testing.T) {
	root := t.TempDir()
	reg := devcouncil.NewRegistry(root, scopedStore(t), enforcingGate(t, root))
	args := map[string]any{"path": "src/a.go", "task_id": "TASK-1", "operation": "delete"}
	verdict := policyCheck(t, reg, args).(map[string]any)
	if allowed, _ := verdict["allowed"].(bool); allowed {
		t.Fatalf("delete of a modify-planned path was allowed: %+v", verdict)
	}
	planned := []dc.PlannedFile{{Path: "src/a.go", AllowedChange: dc.ChangeModify}}
	for _, g := range verify.DetectOrphanDiffGaps("TASK-1", planned, []string{"src/a.go"}, nil) {
		t.Errorf("verification must not call a planned path an orphan: %+v", g)
	}
}

// forbidden_changes entries are spelled once, where the store hands the task
// over: a "./" entry denies the path it names, including creating it while it
// does not exist (forbiddenAlias cannot help there, there is no file yet).
func TestDotSlashForbiddenEntryDeniesAnAbsentPath(t *testing.T) {
	root := t.TempDir()
	client := store.New(testsupport.DCStore(t), filepath.Join(t.TempDir(), "state.sqlite"))
	if _, err := client.ActiveLeases(t.Context()); err != nil {
		t.Fatal(err)
	}
	testsupport.SeedTask(t, client.DB, testsupport.TaskSeed{ID: "TASK-F", Scope: `{"planned_files":[{"path":"secret/*","allowed_change":"create"}],"forbidden_changes":["./secret/x"]}`})
	reg := devcouncil.NewRegistry(root, client, enforcingGate(t, root))
	args := map[string]any{"path": "secret/x", "task_id": "TASK-F", "operation": "create"}
	allowed, d := verdictOf(t, args, policyCheck(t, reg, args))
	if allowed || d.Rule != policy.RuleForbiddenChange {
		t.Errorf("create secret/x with forbidden ./secret/x: allowed=%v rule=%s, want denied by %s", allowed, d.Rule, policy.RuleForbiddenChange)
	}
	other := map[string]any{"path": "secret/y", "task_id": "TASK-F", "operation": "create"}
	if allowed, d := verdictOf(t, other, policyCheck(t, reg, other)); !allowed {
		t.Errorf("secret/y is planned by the glob and not forbidden: %+v", d)
	}
}
