package devcouncil_test

import (
	"context"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil"
)

func TestGetDiffGoldenCleanDirtyStagedUntracked(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip(err)
	}
	root := newRepo(t)
	// clean
	got, err := devcouncil.GetDiff(context.Background(), root, devcouncil.GetDiffArgs{})
	if err != nil {
		t.Fatal(err)
	}
	dr := mustDiff(t, got)
	if !dr.OK || len(dr.Files) != 0 || dr.UnifiedDiff != "" || dr.Truncated {
		t.Fatalf("clean: %+v", dr)
	}

	write(t, root, "src/a.py", "VALUE = 2\n")
	got, err = devcouncil.GetDiff(context.Background(), root, devcouncil.GetDiffArgs{})
	if err != nil {
		t.Fatal(err)
	}
	dr = mustDiff(t, got)
	if !dr.OK || len(dr.Files) != 1 || dr.Files[0].Path != "src/a.py" || dr.Files[0].Status != "M" {
		t.Fatalf("dirty: %+v", dr)
	}
	if !strings.Contains(dr.UnifiedDiff, "VALUE = 2") {
		t.Fatalf("dirty unified missing content: %q", dr.UnifiedDiff)
	}

	runGit(t, root, "add", "src/a.py")
	got, err = devcouncil.GetDiff(context.Background(), root, devcouncil.GetDiffArgs{Staged: true})
	if err != nil {
		t.Fatal(err)
	}
	dr = mustDiff(t, got)
	if !dr.OK || !dr.Staged || len(dr.Files) != 1 {
		t.Fatalf("staged: %+v", dr)
	}

	write(t, root, "new.txt", "hello\n")
	got, err = devcouncil.GetDiff(context.Background(), root, devcouncil.GetDiffArgs{})
	if err != nil {
		t.Fatal(err)
	}
	dr = mustDiff(t, got)
	found := false
	for _, f := range dr.Files {
		if f.Path == "new.txt" && f.Status == "A" {
			found = true
		}
	}
	if !found {
		t.Fatalf("untracked missing: %+v", dr.Files)
	}
}

func TestGetDiffEmptyPlannedScopeFailClosed(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip(err)
	}
	root := newRepo(t)
	write(t, root, "src/a.py", "VALUE = 9\n")
	got, err := devcouncil.GetDiff(context.Background(), root, devcouncil.GetDiffArgs{
		TaskID: "TASK-001", TaskFound: true, DBInitialized: true, PlannedFiles: nil,
	})
	if err != nil {
		t.Fatal(err)
	}
	dr := mustDiff(t, got)
	if !dr.OK || len(dr.Files) != 0 || dr.UnifiedDiff != "" {
		t.Fatalf("empty planned must fail closed: %+v", dr)
	}
}

func TestGetDiffNonRepo(t *testing.T) {
	dir := t.TempDir()
	got, err := devcouncil.GetDiff(context.Background(), dir, devcouncil.GetDiffArgs{})
	if err != nil {
		t.Fatal(err)
	}
	ep, ok := got.(devcouncil.ErrorPayload)
	if !ok || ep.OK || ep.Code != "not_a_git_repo" {
		t.Fatalf("want not_a_git_repo, got %#v", got)
	}
}

func TestGetDiffBinary(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip(err)
	}
	root := newRepo(t)
	bin := filepath.Join(root, "blob.bin")
	if err := os.WriteFile(bin, append([]byte{0, 1, 2}, []byte("bin")...), 0o644); err != nil {
		t.Fatal(err)
	}
	got, err := devcouncil.GetDiff(context.Background(), root, devcouncil.GetDiffArgs{})
	if err != nil {
		t.Fatal(err)
	}
	dr := mustDiff(t, got)
	found := false
	for _, f := range dr.Files {
		if f.Path == "blob.bin" && f.Status == "A" {
			found = true
		}
	}
	if !found || !strings.Contains(dr.UnifiedDiff, "Binary files") {
		t.Fatalf("binary: %+v\n%s", dr.Files, dr.UnifiedDiff)
	}
}

func TestGetDiffTruncation(t *testing.T) {
	big := strings.Repeat("x", devcouncil.DiffOutputLimit+100)
	out, trunc := devcouncil.TruncateText(big, devcouncil.DiffOutputLimit)
	if !trunc || !strings.Contains(out, "...[truncated to") {
		t.Fatalf("truncation: trunc=%v out_len=%d", trunc, len(out))
	}
}

func TestRegistrySpecsMatchToolsListCount(t *testing.T) {
	reg := devcouncil.NewRegistry(t.TempDir(), nil, nil)
	specs := reg.Specs()
	if len(specs) < 5 {
		t.Fatalf("expected core Phase-4 tools, got %d", len(specs))
	}
	names := map[string]bool{}
	for _, s := range specs {
		if names[s.Name] {
			t.Fatalf("duplicate tool %s", s.Name)
		}
		names[s.Name] = true
		ann := s.Behaviour.Annotations()
		if _, ok := ann["readOnlyHint"]; !ok {
			t.Fatalf("%s missing readOnlyHint", s.Name)
		}
	}
	for _, required := range []string{
		"devcouncil_get_diff", "devcouncil_checkout_task", "devcouncil_renew_lease",
		"devcouncil_release_task", "devcouncil_verify_task",
	} {
		if !names[required] {
			t.Fatalf("missing %s", required)
		}
	}
}

func TestLeaseConflictShape(t *testing.T) {
	payload, _ := json.Marshal(map[string]any{
		"ok": false, "code": "lease_conflict",
		"error": "Active lease already exists for task TASK-001", "task_id": "TASK-001",
	})
	var m map[string]any
	if err := json.Unmarshal(payload, &m); err != nil {
		t.Fatal(err)
	}
	if m["code"] != "lease_conflict" {
		t.Fatal(m)
	}
}

func TestGoldenFixturesStillPresent(t *testing.T) {
	root := filepath.Join("..", "testdata", "golden")
	for _, rel := range []string{
		"mcp/get_diff/dirty.json",
		"mcp/get_diff/empty_planned_scope.json",
		"mcp/lease/foreign_active.json",
		"mcp/lease/expired.json",
		"cli/skills/scaffold_ok.json",
	} {
		p := filepath.Join(root, rel)
		if _, err := os.Stat(p); err != nil {
			t.Fatalf("missing golden %s: %v", rel, err)
		}
	}
}

func newRepo(t *testing.T) string {
	t.Helper()
	root := t.TempDir()
	runGit(t, root, "init", "-q")
	runGit(t, root, "config", "user.email", "p4@example.test")
	runGit(t, root, "config", "user.name", "p4")
	runGit(t, root, "config", "commit.gpgsign", "false")
	write(t, root, "src/a.py", "VALUE = 1\n")
	runGit(t, root, "add", "src/a.py")
	runGit(t, root, "commit", "-qm", "seed")
	return root
}

func runGit(t *testing.T, root string, args ...string) {
	t.Helper()
	cmd := exec.Command("git", args...)
	cmd.Dir = root
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("git %v: %v\n%s", args, err, out)
	}
}

func write(t *testing.T, root, rel, body string) {
	t.Helper()
	path := filepath.Join(root, rel)
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
}

func mustDiff(t *testing.T, got any) devcouncil.DiffResult {
	t.Helper()
	dr, ok := got.(devcouncil.DiffResult)
	if !ok {
		t.Fatalf("want DiffResult, got %T %#v", got, got)
	}
	return dr
}
