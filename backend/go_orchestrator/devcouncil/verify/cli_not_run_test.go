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
	"unicode"

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

// storedRows counts what the store holds for a task, through the store
// itself (no sqlite3 CLI needed): gaps through the client, runs through
// `dcstore runs`.
func storedRows(t *testing.T, client *store.Client, taskID string) (gaps, runs int) {
	t.Helper()
	rows, _, err := client.Gaps(context.Background(), taskID)
	if err != nil {
		t.Fatalf("Gaps: %v", err)
	}
	out, err := exec.Command(testsupport.DCStore(t), "--db", client.DB, "runs", "--task", taskID).Output()
	if err != nil {
		t.Fatalf("dcstore runs: %v", err)
	}
	var reply struct {
		Shown int `json:"shown"`
	}
	if err := json.Unmarshal(out, &reply); err != nil {
		t.Fatalf("dcstore runs reply %q: %v", out, err)
	}
	return len(rows), reply.Shown
}

func TestVerifyUnknownTaskIsNotRunAgainstARealStore(t *testing.T) {
	client := store.New(testsupport.DCStore(t), filepath.Join(t.TempDir(), "state.sqlite"))
	t.Cleanup(client.Close)
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
	if gaps, runs := storedRows(t, client, "NO-SUCH"); gaps != 0 || runs != 0 {
		t.Fatalf("a not-run verify wrote %d gap row(s) and %d run row(s)", gaps, runs)
	}
	if stderr != "" {
		t.Fatalf("--json wrote to stderr: %q", stderr)
	}
}

func TestVerifyUnknownTaskHumanModeSaysNotRun(t *testing.T) {
	client := store.New(testsupport.DCStore(t), filepath.Join(t.TempDir(), "state.sqlite"))
	t.Cleanup(client.Close)
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

func TestNotRunHumanLineEscapesControlCharacters(t *testing.T) {
	// The store error carries the binary path verbatim: plant escape, CR, a
	// C1 control and a bidi override in it.
	hostile := "bin\x1b[2J\rEVIL\u0085\u202ex"
	client := store.New(filepath.Join(t.TempDir(), hostile), filepath.Join(t.TempDir(), "state.sqlite"))
	_, _, stderr := runCLIOutput(t, client, "TASK-1", t.TempDir(), verify.GateState{Mode: "enforce"}, false)
	if !strings.HasPrefix(stderr, "verify: NOT RUN (store_error): ") || strings.Count(stderr, "\n") != 1 {
		t.Fatalf("not one NOT RUN line: %q", stderr)
	}
	for _, r := range strings.TrimSuffix(stderr, "\n") {
		if r != ' ' && !unicode.IsPrint(r) {
			t.Fatalf("raw non-printing rune %U reached the terminal: %q", r, stderr)
		}
	}
	if !strings.Contains(stderr, "\\x1b") {
		t.Fatalf("the escape character should be shown as an escape: %q", stderr)
	}
	// A hostile task id in the not-found line is quoted too.
	client = store.New(testsupport.DCStore(t), filepath.Join(t.TempDir(), "state.sqlite"))
	t.Cleanup(client.Close)
	if _, err := client.ActiveLeases(context.Background()); err != nil {
		t.Fatal(err)
	}
	_, _, stderr = runCLIOutput(t, client, "A\x1b[2JB", t.TempDir(), verify.GateState{Mode: "enforce"}, false)
	if strings.ContainsRune(stderr, 0x1b) {
		t.Fatalf("raw ESC in task_not_found line: %q", stderr)
	}
}

func TestNoStoreEnvelopeKeepsAnInvalidGateConfig(t *testing.T) {
	_, stdout, _ := runCLIOutput(t, nil, "TASK-1", t.TempDir(),
		verify.GateState{Mode: "off", Origin: "invalid", Error: "bad gate yaml"}, true)
	env := decodeEnvelope(t, stdout)
	requireNotRun(t, env, "store_unavailable")
	if env["gate_mode_origin"] != "invalid" || env["gate_mode_error"] != "bad gate yaml" {
		t.Fatalf("a judged-run envelope carries the invalid gate config; this one dropped it: %s", stdout)
	}
}

func TestPartialRunKeepsEarnedVerdictsAndNamesTheNotRunTask(t *testing.T) {
	code, stdout, stderr := partialRun(t, true)
	if code != 1 {
		t.Fatalf("exit = %d, want 1", code)
	}
	env := decodeEnvelope(t, stdout)
	if _, top := env["not_run"]; top {
		t.Fatalf("top-level not_run on a run that judged a task: %s", stdout)
	}
	if env["ok"] != false || env["processed_tasks"] != float64(1) {
		t.Fatalf("totals: %s", stdout)
	}
	tasks, _ := env["tasks"].([]any)
	if len(tasks) != 2 {
		t.Fatalf("tasks = %d entries, want the judged task and the not-run one: %s", len(tasks), stdout)
	}
	first, second := tasks[0].(map[string]any), tasks[1].(map[string]any)
	if first["task_id"] != "TASK-A" || first["status"] == "not_run" {
		t.Fatalf("first entry should be TASK-A's verdict: %v", first)
	}
	nr, _ := second["not_run"].(map[string]any)
	if second["task_id"] != "TASK-B" || second["status"] != "not_run" || nr["reason"] != "store_error" {
		t.Fatalf("second entry should be TASK-B not_run/store_error: %v", second)
	}
	if _, has := first["not_run"]; has {
		t.Fatalf("a judged entry carries not_run: %v", first)
	}
	_, human, humanErr := partialRun(t, false)
	if !strings.Contains(human, "TASK-A") || !strings.Contains(humanErr, "NOT RUN (store_error)") {
		t.Fatalf("human: stdout=%q stderr=%q", human, humanErr)
	}
	_ = stderr
}
