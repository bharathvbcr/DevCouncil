package stopgate_test

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/stopgate"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// A verification cut short by its caller's budget is the run whose record
// matters most, and Persist used to write it on the expired context, so the
// timed-out run left nothing behind and the verdict was reported as unrecorded.
// The budget here is 1s against a 2s command.
func TestARunCutShortByItsBudgetIsStillRecordedWithItsTimeoutGap(t *testing.T) {
	root := repoWithWorkingTreeChange(t, "src/app.go", "package app\n")
	client := newStore(t)
	testsupport.SeedTask(t, client.DB, testsupport.TaskSeed{ID: "TASK-BUDGET", Title: "budget",
		Scope: `{"planned_files":[{"path":"src/app.go","allowed_change":"modify"}],"expected_tests":["sleep 2"]}`})

	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	out := stopgate.Run(ctx, stopgate.Input{Root: root, Store: client, TaskID: "TASK-BUDGET", GateMode: "enforce"})

	if out.Decision.Skipped {
		t.Fatalf("the run was not recorded: %+v", out.Decision)
	}
	rows, _, err := client.Gaps(context.Background(), "TASK-BUDGET")
	if err != nil {
		t.Fatal(err)
	}
	for _, g := range rows {
		if g.GapType == "test_failed" && g.Blocking {
			return
		}
	}
	t.Fatalf("no persisted timed-out gap among %d gaps: %+v", len(rows), rows)
}

// The run's evidence names, never values, the environment its commands were
// allowed: the repository's extra names, and the ones it asked for and was
// refused.
func TestARecordedRunNamesTheEnvironmentItWasAllowed(t *testing.T) {
	t.Setenv("TEAM_FLAVOUR", "blue-secret-value")
	root := repoWithWorkingTreeChange(t, "src/app.go", "package app\n")
	if err := os.MkdirAll(filepath.Join(root, ".devcouncil"), 0o755); err != nil {
		t.Fatal(err)
	}
	cfg := "verify:\n  env_allow: [TEAM_FLAVOUR, DEPLOY_TOKEN]\n"
	if err := os.WriteFile(filepath.Join(root, ".devcouncil", "config.yaml"), []byte(cfg), 0o644); err != nil {
		t.Fatal(err)
	}
	client := newStore(t)
	plantTask(t, client, "TASK-ENV", "src/app.go")

	out := stopgate.Run(context.Background(), stopgate.Input{Root: root, Store: client, TaskID: "TASK-ENV", GateMode: "enforce"})
	if out.Decision.Skipped {
		t.Fatalf("run did not complete: %+v", out.Decision)
	}
	raw, err := exec.Command(testsupport.Tool(t, "sqlite3"), client.DB, "SELECT environment_json FROM verification_runs WHERE task_id='TASK-ENV';").CombinedOutput()
	if err != nil {
		t.Fatalf("read evidence: %v %s", err, raw)
	}
	evidence := string(raw)
	for _, want := range []string{`"env_scrub":"allowlist"`, `"env_allow_extra":["TEAM_FLAVOUR"]`, `"env_allow_refused":["DEPLOY_TOKEN"]`} {
		if !strings.Contains(evidence, want) {
			t.Errorf("evidence lacks %s: %s", want, evidence)
		}
	}
	if strings.Contains(evidence, "blue-secret-value") {
		t.Errorf("evidence carries a variable's value: %s", evidence)
	}
}
