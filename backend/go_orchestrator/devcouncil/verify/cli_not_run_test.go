package verify_test

// `devcouncil verify <id>` for an id this store does not hold is "not run",
// never a verdict (ft-74d9ffb6). Each test pins one reason a verify can end
// without judging anything, and that a run that did judge carries no not_run.

import (
	"context"
	"encoding/json"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// runCLIOutput runs RunCLI with os.Stdout and os.Stderr redirected to files
// (no console session is active, so console writes there) and returns the exit
// code and what each stream received.
func runCLIOutput(t *testing.T, client *store.Client, taskID, root string, gate verify.GateState, jsonOut bool) (code int, stdout, stderr string) {
	t.Helper()
	dir := t.TempDir()
	outF, err := os.Create(filepath.Join(dir, "out"))
	if err != nil {
		t.Fatal(err)
	}
	errF, err := os.Create(filepath.Join(dir, "err"))
	if err != nil {
		t.Fatal(err)
	}
	oldOut, oldErr := os.Stdout, os.Stderr
	os.Stdout, os.Stderr = outF, errF
	func() {
		defer func() { os.Stdout, os.Stderr = oldOut, oldErr }()
		code = verify.RunCLI(context.Background(), root, client, taskID, gate, "", "", jsonOut)
	}()
	read := func(f *os.File) string {
		if _, err := f.Seek(0, io.SeekStart); err != nil {
			t.Fatal(err)
		}
		b, err := io.ReadAll(f)
		if err != nil {
			t.Fatal(err)
		}
		_ = f.Close()
		return string(b)
	}
	return code, read(outF), read(errF)
}

func decodeEnvelope(t *testing.T, stdout string) map[string]any {
	t.Helper()
	var env map[string]any
	if err := json.Unmarshal([]byte(stdout), &env); err != nil {
		t.Fatalf("stdout is not one JSON envelope: %v\n%s", err, stdout)
	}
	return env
}

func requireNotRun(t *testing.T, env map[string]any, reason string) {
	t.Helper()
	if env["ok"] != false {
		t.Fatalf("ok = %v, want false: %v", env["ok"], env)
	}
	nr, ok := env["not_run"].(map[string]any)
	if !ok {
		t.Fatalf("envelope has no not_run object: %v", env)
	}
	if nr["reason"] != reason {
		t.Fatalf("not_run.reason = %v, want %q", nr["reason"], reason)
	}
	if got := env["processed_tasks"]; got != nil && got != float64(0) {
		t.Fatalf("processed_tasks = %v, want 0", got)
	}
	for _, k := range []string{"verified_tasks", "blocked_tasks", "degraded_tasks", "completed_without_verification"} {
		if v, present := env[k]; present && v != float64(0) {
			t.Fatalf("%s = %v: a not-run counts as none of the verdicts", k, v)
		}
	}
}

func rowCount(t *testing.T, db, table string) string {
	t.Helper()
	bin, err := exec.LookPath("sqlite3")
	if err != nil {
		t.Skip("sqlite3 CLI not available to count " + table)
	}
	out, err := exec.Command(bin, db, "SELECT COUNT(*) FROM "+table).Output()
	if err != nil {
		t.Fatalf("sqlite3 count %s: %v", table, err)
	}
	return strings.TrimSpace(string(out))
}

func TestVerifyUnknownTaskIsNotRunAgainstARealStore(t *testing.T) {
	db := filepath.Join(t.TempDir(), "state.sqlite")
	client := store.New(testsupport.DCStore(t), db)
	if _, err := client.ActiveLeases(context.Background()); err != nil {
		t.Fatal(err)
	}
	code, stdout, stderr := runCLIOutput(t, client, "NO-SUCH", t.TempDir(), verify.GateState{Mode: "enforce"}, true)
	if code == 0 {
		t.Fatalf("exit = 0 for a verify that did not run:\n%s", stdout)
	}
	env := decodeEnvelope(t, stdout)
	requireNotRun(t, env, "task_not_found")
	if nr := env["not_run"].(map[string]any); nr["task_id"] != "NO-SUCH" {
		t.Fatalf("not_run.task_id = %v", nr["task_id"])
	}
	if env["error"] != "task NO-SUCH not found" {
		t.Fatalf("error text moved: %v", env["error"])
	}
	if env["processed_tasks"] != float64(0) {
		t.Fatalf("processed_tasks = %v, want 0", env["processed_tasks"])
	}
	for _, table := range []string{"verification_runs", "gaps"} {
		if n := rowCount(t, db, table); n != "0" {
			t.Fatalf("%s has %s rows after a not-run verify", table, n)
		}
	}
	if stderr != "" {
		t.Fatalf("--json wrote to stderr: %q", stderr)
	}
}

func TestVerifyUnknownTaskHumanModeSaysNotRun(t *testing.T) {
	client := store.New(testsupport.DCStore(t), filepath.Join(t.TempDir(), "state.sqlite"))
	if _, err := client.ActiveLeases(context.Background()); err != nil {
		t.Fatal(err)
	}
	code, stdout, stderr := runCLIOutput(t, client, "ft-abc123", t.TempDir(), verify.GateState{Mode: "enforce"}, false)
	if code != 1 {
		t.Fatalf("exit = %d, want 1", code)
	}
	want := "verify: NOT RUN (task_not_found): no task \"ft-abc123\" in this repository's task store\n"
	if stderr != want {
		t.Fatalf("stderr = %q, want %q", stderr, want)
	}
	if strings.Contains(stdout, "verified=") {
		t.Fatalf("human mode printed a verdict line for a not-run: %q", stdout)
	}
}

func TestVerifyNilStoreIsNotRunStoreUnavailable(t *testing.T) {
	code, stdout, _ := runCLIOutput(t, nil, "TASK-1", t.TempDir(), verify.GateState{Mode: "enforce"}, true)
	if code != 1 {
		t.Fatalf("exit = %d, want 1", code)
	}
	env := decodeEnvelope(t, stdout)
	requireNotRun(t, env, "store_unavailable")
	if env["error"] != "DevCouncil state is unavailable in this directory." {
		t.Fatalf("error text moved: %v", env["error"])
	}
}

func TestVerifyStoreReadErrorIsNotRunStoreError(t *testing.T) {
	client := store.New(filepath.Join(t.TempDir(), "no-such-dcstore-binary"), filepath.Join(t.TempDir(), "state.sqlite"))
	code, stdout, _ := runCLIOutput(t, client, "TASK-1", t.TempDir(), verify.GateState{Mode: "enforce"}, true)
	if code != 1 {
		t.Fatalf("exit = %d, want 1", code)
	}
	env := decodeEnvelope(t, stdout)
	requireNotRun(t, env, "store_error")
	if env["error"] == "" || env["error"] == nil {
		t.Fatal("a store error must keep its text in error")
	}
}

func TestVerifiedAndBlockedRunsCarryNoNotRun(t *testing.T) {
	t.Setenv(verify.AllowNoRigorEnv, "1")
	root := modifiedRepo(t)
	for _, tc := range []struct {
		name, id, command, mode string
		blocked                 bool
	}{
		{"blocked", "TASK-BLOCKED", "false", "enforce", true},
		{"judged", "TASK-JUDGED", "true", "off", false},
	} {
		t.Run(tc.name, func(t *testing.T) {
			client := plantRepoTask(t, root, tc.id, tc.command)
			code, stdout, _ := runCLIOutput(t, client, tc.id, root, verify.GateState{Mode: tc.mode}, true)
			env := decodeEnvelope(t, stdout)
			if _, present := env["not_run"]; present {
				t.Fatalf("a run that judged the task carries not_run: %s", stdout)
			}
			if env["processed_tasks"] != float64(1) {
				t.Fatalf("processed_tasks = %v, want 1: %s", env["processed_tasks"], stdout)
			}
			if tc.blocked && (code != 1 || env["blocked_tasks"] != float64(1)) {
				t.Fatalf("blocked run: exit %d envelope %s", code, stdout)
			}
		})
	}
}
