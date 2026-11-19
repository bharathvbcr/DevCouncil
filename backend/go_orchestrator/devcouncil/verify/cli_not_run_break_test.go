package verify_test

// BREAK tests (uncommitted) for the typed not_run envelope (ft-74d9ffb6 lane R attack).

import (
	"context"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// A multi-task run (no id: every ready task) where the second task cannot be
// read. The first task was judged.
func partialRun(t *testing.T, jsonOut bool) (int, string, string) {
	t.Helper()
	t.Setenv(verify.AllowNoRigorEnv, "1")
	sqlite, err := exec.LookPath("sqlite3")
	if err != nil {
		t.Skip("sqlite3 CLI not available to corrupt a task row")
	}
	root := modifiedRepo(t)
	client := plantRepoTask(t, root, "TASK-A", "true")
	testsupport.SeedTask(t, client.DB, testsupport.TaskSeed{ID: "TASK-B", Title: "b", Priority: 3})
	if out, err := exec.Command(sqlite, client.DB,
		"INSERT INTO task_scope_appends(task_id, planned_files_json) VALUES('TASK-B','not json')").CombinedOutput(); err != nil {
		t.Fatalf("corrupt: %v %s", err, out)
	}
	return runCLIOutput(t, client, "", root, verify.GateState{Mode: "off"}, jsonOut)
}

func TestPartialRunJSON(t *testing.T) {
	code, stdout, stderr := partialRun(t, true)
	t.Logf("exit=%d\nstdout=%s\nstderr=%s", code, stdout, stderr)
	env := decodeEnvelope(t, stdout)
	_, hasNotRun := env["not_run"]
	if hasNotRun && env["processed_tasks"] != float64(0) {
		t.Fatalf("a run that judged %v task(s) also says not_run: not_run is documented absent on any run that judged something", env["processed_tasks"])
	}
}

func TestPartialRunHumanKeepsTheVerdictItEarned(t *testing.T) {
	_, stdout, stderr := partialRun(t, false)
	t.Logf("stdout=%q stderr=%q", stdout, stderr)
	if !strings.Contains(stdout, "TASK-A") {
		t.Fatalf("TASK-A was judged and its verdict is nowhere in human output; stdout=%q stderr=%q", stdout, stderr)
	}
}

var _ = filepath.Join
var _ = context.Background
var _ *store.Client
