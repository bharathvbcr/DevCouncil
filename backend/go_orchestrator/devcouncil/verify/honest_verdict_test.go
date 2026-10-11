package verify_test

// A check that could not run must never report the same result as one that ran
// and passed. Each test below pins one way the verdict used to say "verified"
// without the evidence (ft-b9ef3ccf, ft-1ca67c6c, ft-a4b3e5f0).

import (
	"context"
	"encoding/json"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/dcverify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

func failingRun(string) verify.CommandOutcome {
	return verify.CommandOutcome{ExitCode: 1, Stderr: "FAIL: boom"}
}

func TestAdvisoryDoesNotHideAFailingCommand(t *testing.T) {
	in := verificationInput(failingRun)
	in.GateMode = "advisory"
	in.AllowNoRigor = true
	gaps, meta := verify.Run(context.Background(), in)
	for name, got := range map[string]struct {
		status   string
		passed   bool
		blocking int
		advisory int
		reasons  int
	}{
		"mcp": func() (r struct {
			status   string
			passed   bool
			blocking int
			advisory int
			reasons  int
		}) {
			m := verify.ToMCP(in.Task.ID, gaps, meta)
			r.status, r.passed, r.blocking, r.advisory, r.reasons = m.Status, m.Passed, len(m.BlockingGaps), len(m.AdvisoryActions), len(m.DegradedReasons)
			for _, g := range m.BlockingGaps {
				if !g.Blocking {
					t.Errorf("mcp: blocking_gaps carries a non-blocking gap")
				}
			}
			for _, a := range m.AdvisoryActions {
				if a.Blocking {
					t.Errorf("mcp: an advisory action says blocking:true: %+v", a)
				}
			}
			return
		}(),
		"cli": func() (r struct {
			status   string
			passed   bool
			blocking int
			advisory int
			reasons  int
		}) {
			c := verify.ToCLITask(in.Task.ID, gaps, meta)
			r.status, r.blocking, r.advisory, r.reasons = c.Status, c.BlockingGapCount, len(c.AdvisoryActions), len(c.DegradedReasons)
			return
		}(),
	} {
		if got.status != "degraded" || got.passed || got.blocking != 0 || got.advisory != 1 || got.reasons == 0 {
			t.Errorf("%s: a demoted failing test must be degraded, not passed, listed as advisory: %+v", name, got)
		}
	}
}

func TestEnforceWithoutDcverifyIsDegradedNotVerified(t *testing.T) {
	in := verificationInput(func(string) verify.CommandOutcome { return verify.CommandOutcome{} })
	gaps, meta := verify.Run(context.Background(), in)
	result := verify.ToMCP(in.Task.ID, gaps, meta)
	if result.Passed || result.Status != "degraded" || len(result.DegradedReasons) == 0 {
		t.Fatalf("scans never ran, yet: %+v", result)
	}
	in.AllowNoRigor = true
	gaps, meta = verify.Run(context.Background(), in)
	if result = verify.ToMCP(in.Task.ID, gaps, meta); !result.Passed || result.Status != "verified" {
		t.Fatalf("explicit opt-out must restore the old verdict: %+v", result)
	}
	if result.RigorSkippedReason == "" {
		t.Fatal("the opt-out must still say rigor was skipped")
	}
}

func TestAnUnreadableGitStateIsABlockingGapInEveryMode(t *testing.T) {
	for _, mode := range []string{"enforce", "advisory"} {
		in := verificationInput(func(string) verify.CommandOutcome { return verify.CommandOutcome{} })
		in.GateMode, in.AllowNoRigor = mode, true
		in.GitErr = errors.New("git diff HEAD: exit status 128: fatal: bad object")
		in.ChangedFiles, in.DiffContent, in.DiffEmpty, in.WorkPresent = nil, "", true, false
		gaps, meta := verify.Run(context.Background(), in)
		result := verify.ToMCP(in.Task.ID, gaps, meta)
		got := gapsByType(gaps)["rigor_check_unavailable"]
		if result.Passed || result.Status != "blocked" || len(got) != 1 || !got[0].Blocking {
			t.Fatalf("%s: %+v gaps=%+v", mode, result, gaps)
		}
		if !strings.Contains(got[0].Description, "bad object") {
			t.Errorf("git's own reason must reach the gap: %q", got[0].Description)
		}
	}
}

func TestCollectGitStateOutsideARepoReportsTheError(t *testing.T) {
	_, _, _, err := verify.CollectGitState(context.Background(), t.TempDir())
	if err == nil {
		t.Fatal("a directory git cannot read must not collect as an empty diff")
	}
}

func TestAnUnmatchedCoverageProfileIsNotMeasured(t *testing.T) {
	profile := filepath.Join(t.TempDir(), "lcov.info")
	if err := os.WriteFile(profile, []byte("SF:src/other.rs\nDA:1,1\nend_of_record\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	in := withRigor(t, "TASK-UNMEASURED", "enforce")
	in.CoveragePath = profile
	gaps, meta := verify.Run(context.Background(), in)
	result := verify.ToMCP("TASK-UNMEASURED", gaps, meta)
	if result.CoverageMeasured {
		t.Fatal("a profile that says nothing about the changed file measured nothing")
	}
	if !strings.Contains(result.CoverageSkippedReason, "src/handler.rs") {
		t.Fatalf("the unmeasured file must be named: %q", result.CoverageSkippedReason)
	}
	_ = dcverify.GateDiffCoverage
}

// readRun reads the stored run back with the sqlite3 CLI: the store has no
// read command for runs, and the point is what is on disk.
func readRun(t *testing.T, db, taskID string) (commands string, started, finished time.Time) {
	t.Helper()
	bin, err := exec.LookPath("sqlite3")
	if err != nil {
		t.Skip("sqlite3 CLI not available to read verification_runs")
	}
	out, err := exec.Command(bin, "-separator", "\x1f", db,
		"SELECT commands_json, started_at, finished_at FROM verification_runs WHERE task_id='"+taskID+"'").Output()
	if err != nil {
		t.Fatalf("sqlite3: %v", err)
	}
	parts := strings.Split(strings.TrimSpace(string(out)), "\x1f")
	if len(parts) != 3 {
		t.Fatalf("want exactly one stored run, got %q", out)
	}
	started, err1 := time.Parse(time.RFC3339Nano, parts[1])
	finished, err2 := time.Parse(time.RFC3339Nano, parts[2])
	if err1 != nil || err2 != nil {
		t.Fatalf("timestamps %q: %v %v", parts[1:], err1, err2)
	}
	return parts[0], started, finished
}

// plantRepoTask makes a git repository with one committed file modified in the
// working tree, a store, and one ready task that expects the given command.
func plantRepoTask(t *testing.T, root, id, command string) *store.Client {
	t.Helper()
	client := store.New(testsupport.DCStore(t), filepath.Join(t.TempDir(), "state.sqlite"))
	if _, err := client.ActiveLeases(context.Background()); err != nil {
		t.Fatal(err)
	}
	tests, _ := json.Marshal([]string{command})
	sql := "INSERT INTO tasks (id,title,description,planned_files_json,expected_tests_json,status) VALUES ('" +
		id + "','t','','[{\"path\":\"a.go\",\"allowed_change\":\"modify\"}]','" +
		strings.ReplaceAll(string(tests), "'", "''") + "','ready');"
	if out, err := exec.Command(testsupport.Tool(t, "sqlite3"), client.DB, sql).CombinedOutput(); err != nil {
		t.Fatalf("task fixture: %v %s", err, out)
	}
	return client
}

func modifiedRepo(t *testing.T) string {
	t.Helper()
	git := testsupport.Tool(t, "git")
	root := t.TempDir()
	run := func(args ...string) {
		cmd := exec.Command(git, args...)
		cmd.Dir = root
		cmd.Env = append(os.Environ(), "GIT_AUTHOR_NAME=t", "GIT_AUTHOR_EMAIL=t@e.invalid",
			"GIT_COMMITTER_NAME=t", "GIT_COMMITTER_EMAIL=t@e.invalid")
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v %s", args, err, out)
		}
	}
	run("init", "-q", root)
	if err := os.WriteFile(filepath.Join(root, "a.go"), []byte("package a\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	run("add", ".")
	run("commit", "-qm", "base")
	if err := os.WriteFile(filepath.Join(root, "a.go"), []byte("package a\n\nfunc A() {}\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	return root
}

func TestVerifyTaskPersistsTheCommandsTheRunExecuted(t *testing.T) {
	t.Setenv(verify.AllowNoRigorEnv, "1")
	ctx := context.Background()
	root := modifiedRepo(t)
	client := plantRepoTask(t, root, "TASK-EVIDENCE", "echo boom >&2; false")
	result, gaps, err := verify.VerifyTask(ctx, root, client, "TASK-EVIDENCE", verify.GateState{Mode: "enforce"}, "", "")
	if err != nil {
		t.Fatal(err)
	}
	if result.Status != "blocked" {
		t.Fatalf("status=%s", result.Status)
	}
	commandsJSON, started, finished := readRun(t, client.DB, "TASK-EVIDENCE")
	var cmds []verify.CommandRecord
	if err := json.Unmarshal([]byte(commandsJSON), &cmds); err != nil || len(cmds) != 1 {
		t.Fatalf("commands_json=%s err=%v", commandsJSON, err)
	}
	if cmds[0].Command != "echo boom >&2; false" || cmds[0].ExitCode != 1 || !strings.Contains(cmds[0].Tail, "boom") {
		t.Fatalf("command evidence lost: %+v", cmds[0])
	}
	if !finished.After(started) {
		t.Fatalf("a run that executed commands has finished_at %v not after started_at %v", finished, started)
	}
	failed := gapsByType(gaps)["test_failed"]
	if len(failed) != 1 || !strings.Contains(failed[0].Evidence[0], "exit code 1") {
		t.Fatalf("test_failed must carry the exit code: %+v", failed)
	}
	rows, _, err := client.Gaps(ctx, "TASK-EVIDENCE")
	if err != nil || len(rows) == 0 {
		t.Fatalf("gaps=%v err=%v", rows, err)
	}
}

func TestVerifyTaskWithUnreadableGitIsBlockedNotAnEmptyDiff(t *testing.T) {
	t.Setenv(verify.AllowNoRigorEnv, "1")
	ctx := context.Background()
	notARepo := t.TempDir()
	client := plantRepoTask(t, notARepo, "TASK-NOGIT", "true")
	for _, mode := range []string{"enforce", "advisory"} {
		result, gaps, err := verify.VerifyTask(ctx, notARepo, client, "TASK-NOGIT", verify.GateState{Mode: mode}, "", "")
		if err != nil {
			t.Fatal(err)
		}
		got := gapsByType(gaps)["rigor_check_unavailable"]
		if result.Passed || result.Status != "blocked" || len(got) != 1 || !got[0].Blocking {
			t.Fatalf("%s: git could not be read, yet %+v gaps=%+v", mode, result, gaps)
		}
	}
}

func TestPersistedGapsFromASkippedRunAreNotBlocking(t *testing.T) {
	ctx := context.Background()
	client := store.New(testsupport.DCStore(t), filepath.Join(t.TempDir(), "state.sqlite"))
	in := verificationInput(failingRun)
	in.GateMode = "off"
	gaps, meta := verify.Run(ctx, in)
	if err := verify.Persist(ctx, client, in.Task.ID, gaps, meta, "skipped"); err != nil {
		t.Fatal(err)
	}
	rows, _, err := client.Gaps(ctx, in.Task.ID)
	if err != nil {
		t.Fatal(err)
	}
	for _, r := range rows {
		if r.Blocking {
			t.Errorf("a run that verified nothing left a blocking gap: %+v", r)
		}
	}
}

// A changed file the syntax tree could not read took the weaker line checks.
// If those found nothing, the run is not clean: it is degraded, and says which
// file and why.
func TestAFileTheTreeCouldNotReadMakesTheRunDegraded(t *testing.T) {
	root := t.TempDir()
	if err := os.MkdirAll(filepath.Join(root, "src"), 0o755); err != nil {
		t.Fatal(err)
	}
	source := "fn handle() {\n    let x = ;\n}\n"
	if err := os.WriteFile(filepath.Join(root, "src/handler.rs"), []byte(source), 0o600); err != nil {
		t.Fatal(err)
	}
	diff := "diff --git a/src/handler.rs b/src/handler.rs\n--- a/src/handler.rs\n+++ b/src/handler.rs\n" +
		"@@ -1,0 +1,3 @@\n+fn handle() {\n+    let x = ;\n+}\n"
	in := withRigor(t, "TASK-SYNTAX", "enforce")
	in.Rigor = dcverify.New(testsupport.DCVerify(t), root)
	in.DiffContent = diff
	gaps, meta := verify.Run(context.Background(), in)
	result := verify.ToMCP("TASK-SYNTAX", gaps, meta)
	if result.Passed || result.Status != "degraded" {
		t.Fatalf("a syntax-error file was called clean: status=%s passed=%v gaps=%+v", result.Status, result.Passed, gaps)
	}
	joined := strings.Join(result.DegradedReasons, " ")
	if !strings.Contains(joined, "src/handler.rs") || !strings.Contains(joined, "syntax error") {
		t.Fatalf("degraded reasons must name the file and why: %q", joined)
	}
}
