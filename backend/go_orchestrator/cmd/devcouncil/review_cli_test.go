package main

import (
	"context"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// reviewRepo is a git repository with one commit and a task store holding
// TASK-1, with the test-built dcstore first on PATH.
func reviewRepo(t *testing.T) string {
	t.Helper()
	bin := testsupport.DCStore(t)
	t.Setenv("PATH", filepath.Dir(bin)+string(os.PathListSeparator)+os.Getenv("PATH"))
	git := testsupport.Tool(t, "git")
	root := t.TempDir()
	for _, args := range [][]string{{"init", "-q", root}, {"-C", root, "commit", "-q", "--allow-empty", "-m", "base"}} {
		cmd := exec.Command(git, args...)
		cmd.Env = append(os.Environ(), "GIT_AUTHOR_NAME=t", "GIT_AUTHOR_EMAIL=t@e.invalid",
			"GIT_COMMITTER_NAME=t", "GIT_COMMITTER_EMAIL=t@e.invalid")
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v %s", args, err, out)
		}
	}
	if err := os.WriteFile(filepath.Join(root, "a.go"), []byte("package a\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	if code, out := cli(t, "task", "create", "--id", "TASK-1", "--title", "t", "--project-root", root); code != 0 {
		t.Fatalf("task create: %d %s", code, out)
	}
	return root
}

// cli runs one host command and returns its exit code and everything it wrote.
func cli(t *testing.T, args ...string) (int, string) {
	t.Helper()
	stdout, restoreOut := swapStdout(t)
	stderr, restoreErr := swapStderr(t)
	code := dispatch(args)
	restoreErr()
	restoreOut()
	return code, stdout.String() + stderr.String()
}

func writeRecord(t *testing.T, subject string) string {
	t.Helper()
	path := filepath.Join(t.TempDir(), "review.json")
	record := map[string]any{
		"schema": 1, "task_id": "TASK-1", "acceptance_criterion_id": "AC-1",
		"subject_hash": subject, "harness": "test", "criterion_sha256": strings.Repeat("e", 64),
		"rubric":    map[string]any{"path": "docs/rubric.md", "sha256": strings.Repeat("c", 64)},
		"isolation": map[string]any{"probe": "passed"},
		"reviewers": []any{
			map[string]any{"id": "a", "persona": "critic", "status": "completed"},
			map[string]any{"id": "b", "persona": "critic", "status": "completed"},
		},
		"findings": []any{},
	}
	data, err := json.Marshal(record)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, data, 0o600); err != nil {
		t.Fatal(err)
	}
	return path
}

// reviewKey makes a signing key with `review keygen` and returns its path.
func reviewKey(t *testing.T) string {
	t.Helper()
	keyPath := filepath.Join(t.TempDir(), "k.json")
	code, out := cli(t, "review", "keygen", "--key-id", "test", "--out", keyPath)
	if code != 0 {
		t.Fatalf("keygen: %d %s", code, out)
	}
	info, err := os.Stat(keyPath)
	if err != nil {
		t.Fatal(err)
	}
	if info.Mode().Perm() != 0o600 {
		t.Fatalf("key file mode %o, want 600", info.Mode().Perm())
	}
	return keyPath
}

func TestReviewSubjectNamesTheWorkingTree(t *testing.T) {
	root := reviewRepo(t)
	want, err := verify.WorkingTreeSubject(context.Background(), root)
	if err != nil {
		t.Fatal(err)
	}
	code, out := cli(t, "review", "subject", "--project-root", root)
	if code != 0 || strings.TrimSpace(out) != want {
		t.Fatalf("exit %d, out %q, want %q", code, out, want)
	}
}

// A record round-trips, and the receipt says whether verify will use it: only
// while the working tree is the content reviewed.
func TestReviewRecordAndListRoundTrip(t *testing.T) {
	root := reviewRepo(t)
	subject, err := verify.WorkingTreeSubject(context.Background(), root)
	if err != nil {
		t.Fatal(err)
	}
	key := reviewKey(t)
	code, out := cli(t, "review", "record", "--file", writeRecord(t, subject), "--sign-key", key, "--json", "--project-root", root)
	if code != 0 {
		t.Fatalf("record: %d %s", code, out)
	}
	var receipt struct {
		OK      bool  `json:"ok"`
		ID      int64 `json:"review_id"`
		Matches bool  `json:"matches_working_tree"`
	}
	if err := json.Unmarshal([]byte(out), &receipt); err != nil || !receipt.OK || receipt.ID <= 0 || !receipt.Matches {
		t.Fatalf("receipt %s (%v)", out, err)
	}

	if err := os.WriteFile(filepath.Join(root, "a.go"), []byte("package a // edited\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	code, out = cli(t, "review", "record", "--file", writeRecord(t, subject), "--sign-key", key, "--project-root", root)
	if code != 0 || !strings.Contains(out, "verify will not use this review") {
		t.Fatalf("a record of content that has moved was not flagged: %d %s", code, out)
	}

	code, out = cli(t, "review", "list", "--task", "TASK-1", "--criterion", "AC-1", "--json", "--project-root", root)
	if code != 0 {
		t.Fatalf("list: %d %s", code, out)
	}
	var listed struct {
		Reviews []struct {
			ID      int64           `json:"review_id"`
			Subject string          `json:"subject_hash"`
			Record  json.RawMessage `json:"record"`
		} `json:"reviews"`
	}
	if err := json.Unmarshal([]byte(out), &listed); err != nil || len(listed.Reviews) != 2 ||
		listed.Reviews[1].ID != receipt.ID || listed.Reviews[0].Subject != subject || len(listed.Reviews[0].Record) == 0 {
		t.Fatalf("list %s (%v)", out, err)
	}
}

// An unsigned record with no --sign-key is refused: the stored record must
// carry a signature verify can check.
func TestAnUnsignedRecordWithoutASigningKeyIsRefused(t *testing.T) {
	root := reviewRepo(t)
	subject, err := verify.WorkingTreeSubject(context.Background(), root)
	if err != nil {
		t.Fatal(err)
	}
	code, out := cli(t, "review", "record", "--file", writeRecord(t, subject), "--project-root", root)
	if code != 1 || !(strings.Contains(out, "signature") || strings.Contains(out, "key_id")) {
		t.Fatalf("an unsigned record was not refused: exit %d: %s", code, out)
	}
	if code, out := cli(t, "review", "list", "--task", "TASK-1", "--project-root", root); code != 0 || !strings.Contains(out, "no reviews recorded") {
		t.Fatalf("exit %d: %s", code, out)
	}
}

func TestAMalformedReviewIsRefusedAndNothingIsRecorded(t *testing.T) {
	root := reviewRepo(t)
	bad := filepath.Join(t.TempDir(), "bad.json")
	if err := os.WriteFile(bad, []byte(`{"schema":1,"task_id":"TASK-1","approved":true}`), 0o600); err != nil {
		t.Fatal(err)
	}
	if code, out := cli(t, "review", "record", "--file", bad, "--project-root", root); code != 1 || !strings.Contains(out, "approved") {
		t.Fatalf("exit %d: %s", code, out)
	}
	if code, out := cli(t, "review", "list", "--task", "TASK-1", "--project-root", root); code != 0 || !strings.Contains(out, "no reviews recorded") {
		t.Fatalf("exit %d: %s", code, out)
	}
}

func TestReviewUsageErrors(t *testing.T) {
	for _, args := range [][]string{
		{"review"},
		{"review", "approve"},
		{"review", "record"},
		{"review", "record", "--file"},
		{"review", "list"},
		{"review", "subject", "extra"},
	} {
		if code, out := cli(t, args...); code != 2 {
			t.Errorf("%v: exit %d, want 2: %s", args, code, out)
		}
	}
}
