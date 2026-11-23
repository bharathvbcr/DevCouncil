package verify_test

// Adversarial pins for the typed not_run envelope (ft-74d9ffb6 lane R attack).
// Every test here passes against the branch and pins a behaviour that held.

import (
	"context"
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

func runCLICtx(t *testing.T, ctx context.Context, client *store.Client, taskID, root string, jsonOut bool) (code int, stdout, stderr string) {
	t.Helper()
	dir := t.TempDir()
	outF, _ := os.Create(filepath.Join(dir, "out"))
	errF, _ := os.Create(filepath.Join(dir, "err"))
	oldOut, oldErr := os.Stdout, os.Stderr
	os.Stdout, os.Stderr = outF, errF
	func() {
		defer func() { os.Stdout, os.Stderr = oldOut, oldErr }()
		code = verify.RunCLI(ctx, root, client, taskID, verify.GateState{Mode: "enforce"}, "", "", jsonOut)
	}()
	read := func(f *os.File) string {
		_, _ = f.Seek(0, io.SeekStart)
		b, _ := io.ReadAll(f)
		_ = f.Close()
		return string(b)
	}
	return code, read(outF), read(errF)
}

func emptyStore(t *testing.T) *store.Client {
	t.Helper()
	client := store.New(testsupport.DCStore(t), filepath.Join(t.TempDir(), "state.sqlite"))
	if _, err := client.ActiveLeases(context.Background()); err != nil {
		t.Fatal(err)
	}
	return client
}

// An id with quotes, newlines, bidi controls, a 10 KB length or a NUL-free
// escape sequence round-trips through the JSON envelope as one valid object.
func TestNotRunOddIDsRoundTripThroughJSON(t *testing.T) {
	client := emptyStore(t)
	for name, id := range map[string]string{
		"space": "a b", "quote": "a\"b'c\\", "newline": "a\nb", "esc": "a\x1b[31mRED\x1b[0m\a",
		"bidi": "a\u202eb\u2028c", "unicode": "日本語", "big": strings.Repeat("x", 10<<10),
	} {
		t.Run(name, func(t *testing.T) {
			code, stdout, _ := runCLICtx(t, context.Background(), client, id, t.TempDir(), true)
			if code != 1 {
				t.Fatalf("exit = %d", code)
			}
			env := decodeEnvelope(t, stdout)
			requireNotRun(t, env, "task_not_found")
			if got := env["not_run"].(map[string]any)["task_id"]; got != id {
				t.Fatalf("task_id did not round-trip: %q", got)
			}
		})
	}
}

// Human mode quotes the id: no raw control character or newline reaches the
// terminal. (The 10 KB id is cut to about 4 KB by the console in the real
// binary; RunCLI itself does not bound it.)
func TestNotRunHumanLineNeverCarriesRawControlCharacters(t *testing.T) {
	client := emptyStore(t)
	for _, id := range []string{"a\x1b[31mRED\x1b[0m\a", "a\nb", "a\rb", "a\u202eb\u0085c", strings.Repeat("x", 10<<10)} {
		code, stdout, stderr := runCLICtx(t, context.Background(), client, id, t.TempDir(), false)
		if code != 1 {
			t.Fatalf("exit = %d", code)
		}
		if strings.Contains(stdout, "verified=") {
			t.Fatalf("a verdict line for a not-run: %q", stdout)
		}
		body := strings.TrimSuffix(stderr, "\n")
		for _, r := range body {
			if r < 0x20 || r == 0x7f || r == 0x85 || r == 0x202e || r == 0x2028 {
				t.Fatalf("raw control character %U in %q", r, stderr)
			}
		}
	}
}

// A task row the store cannot read is store_error, not task_not_found, and
// judges nothing.
func TestUnreadableTaskRowIsStoreErrorNotTaskNotFound(t *testing.T) {
	sqlite, err := exec.LookPath("sqlite3")
	if err != nil {
		t.Skip("sqlite3 CLI not available")
	}
	client := emptyStore(t)
	testsupport.SeedTask(t, client.DB, testsupport.TaskSeed{ID: "TASK-BAD", Title: "b"})
	if out, err := exec.Command(sqlite, client.DB,
		"INSERT INTO task_scope_appends(task_id, planned_files_json) VALUES('TASK-BAD','not json')").CombinedOutput(); err != nil {
		t.Fatalf("corrupt: %v %s", err, out)
	}
	code, stdout, _ := runCLICtx(t, context.Background(), client, "TASK-BAD", t.TempDir(), true)
	if code != 1 {
		t.Fatalf("exit = %d", code)
	}
	requireNotRun(t, decodeEnvelope(t, stdout), "store_error")
}

// A binary that never answers or cannot start is store_error, never
// task_not_found, and never a verdict.
func TestDeadStoreBinaryIsNeverTaskNotFound(t *testing.T) {
	client := store.New("/bin/false", filepath.Join(t.TempDir(), "state.sqlite"))
	code, stdout, _ := runCLICtx(t, context.Background(), client, "NOPE", t.TempDir(), true)
	if code != 1 {
		t.Fatalf("exit = %d", code)
	}
	env := decodeEnvelope(t, stdout)
	requireNotRun(t, env, "store_error")
	if env["verified_tasks"] != float64(0) {
		t.Fatalf("verified_tasks = %v", env["verified_tasks"])
	}
}

// A cancelled context never yields exit 0 or a verdict count.
func TestCancelledContextNeverVerifies(t *testing.T) {
	client := emptyStore(t)
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	code, stdout, _ := runCLICtx(t, ctx, client, "NOPE", t.TempDir(), true)
	if code == 0 {
		t.Fatalf("exit 0 under a cancelled context: %s", stdout)
	}
	env := decodeEnvelope(t, stdout)
	if env["ok"] != false || env["verified_tasks"] != float64(0) {
		t.Fatalf("cancelled run reported a verdict: %s", stdout)
	}
}

// No ready task and no id: exits 1, ok false, no verdict counts. (It carries
// no not_run, which is the untyped gap recorded in the attack report.)
func TestNoTasksToVerifyExitsNonZeroWithNoCounts(t *testing.T) {
	code, stdout, _ := runCLICtx(t, context.Background(), emptyStore(t), "", t.TempDir(), true)
	if code != 1 {
		t.Fatalf("exit = %d", code)
	}
	env := decodeEnvelope(t, stdout)
	if env["ok"] != false || env["error"] != "No tasks found to verify." {
		t.Fatalf("envelope = %s", stdout)
	}
	if _, has := env["verified_tasks"]; has {
		t.Fatalf("a run that never started carries verdict counters: %s", stdout)
	}
}
