package devcouncil_test

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

func seedRepoWithTask(t *testing.T, taskID, plannedPath string) (string, *store.Client) {
	t.Helper()
	root := t.TempDir()
	if err := os.MkdirAll(filepath.Join(root, ".devcouncil"), 0o755); err != nil {
		t.Fatal(err)
	}
	dbPath := filepath.Join(root, ".devcouncil", "state.sqlite")
	client := store.New(testsupport.DCStore(t), dbPath)
	t.Cleanup(client.Close)
	if _, err := client.ActiveLeases(context.Background()); err != nil {
		t.Fatal(err)
	}
	testsupport.SeedTask(t, dbPath, testsupport.TaskSeed{
		ID:     taskID,
		Title:  "task " + taskID,
		Status: "ready",
		Scope:  `{"planned_files":[{"path":"` + plannedPath + `","allowed_change":"modify"}]}`,
	})
	return root, client
}

func TestMultiRepoRoutingViaRepoPathAndRootAlias(t *testing.T) {
	repoA, storeA := seedRepoWithTask(t, "TASK-A", "a.go")
	repoB, storeB := seedRepoWithTask(t, "TASK-B", "b.go")
	gateA := enforcingGate(t, repoA)
	gateB := enforcingGate(t, repoB)

	builds := 0
	pool := devcouncil.NewRegistryPool(8, func(root string) (*devcouncil.Registry, error) {
		builds++
		switch filepath.Clean(root) {
		case filepath.Clean(repoA):
			return devcouncil.NewRegistry(repoA, storeA, gateA), nil
		case filepath.Clean(repoB):
			return devcouncil.NewRegistry(repoB, storeB, gateB), nil
		default:
			return devcouncil.NewRegistry(root, nil, enforcingGate(t, root)), nil
		}
	})
	defer pool.Close()

	reg := devcouncil.NewRegistry(repoA, storeA, gateA)
	reg.ResolveRepo = pool.Get

	// 1. Omitted repo_path/root routes to default repoA (backward compatibility).
	out, err := reg.Call(context.Background(), "devcouncil_next_task", map[string]any{})
	if err != nil {
		t.Fatal(err)
	}
	m := out.(map[string]any)
	if m["ok"] != true || m["task_id"] != "TASK-A" {
		t.Fatalf("default next_task = %+v, want TASK-A", m)
	}

	// 2. repo_path pointing at repoB routes to repoB's store and gate.
	out, err = reg.Call(context.Background(), "devcouncil_next_task", map[string]any{"repo_path": repoB})
	if err != nil {
		t.Fatal(err)
	}
	m = out.(map[string]any)
	if m["ok"] != true || m["task_id"] != "TASK-B" {
		t.Fatalf("repo_path=repoB next_task = %+v, want TASK-B", m)
	}

	// 3. root alias pointing at repoB also hits the cached pool entry.
	out, err = reg.Call(context.Background(), "devcouncil_policy_check_write", map[string]any{
		"path":    "b.go",
		"task_id": "TASK-B",
		"root":    repoB,
	})
	if err != nil {
		t.Fatal(err)
	}
	m = out.(map[string]any)
	if m["ok"] != true || m["allowed"] != true {
		t.Fatalf("root=repoB policy_check_write b.go = %+v, want allowed=true", m)
	}
	if builds != 1 {
		t.Fatalf("pool built repoB %d times, want 1 (cached)", builds)
	}

	// 4. repoB does not leak TASK-A's planned scope.
	out, err = reg.Call(context.Background(), "devcouncil_policy_check_write", map[string]any{
		"path":      "a.go",
		"task_id":   "TASK-B",
		"repo_path": repoB,
	})
	if err != nil {
		t.Fatal(err)
	}
	m = out.(map[string]any)
	if m["ok"] != true || m["allowed"] != false {
		t.Fatalf("repoB policy_check_write a.go under TASK-B = %+v, want allowed=false", m)
	}

	// 5. Passing both repo_path and root pointing to the same repo succeeds.
	out, err = reg.Call(context.Background(), "devcouncil_next_task", map[string]any{
		"repo_path": repoB,
		"root":      repoB + string(filepath.Separator),
	})
	if err != nil {
		t.Fatal(err)
	}
	m = out.(map[string]any)
	if m["ok"] != true || m["task_id"] != "TASK-B" {
		t.Fatalf("agreeing repo_path+root = %+v, want TASK-B", m)
	}
}

func TestMultiRepoValidationFailsClosed(t *testing.T) {
	repoA := t.TempDir()
	repoB := t.TempDir()
	notADir := filepath.Join(t.TempDir(), "file.txt")
	if err := os.WriteFile(notADir, []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}

	reg := devcouncil.NewRegistry(repoA, nil, nil)

	cases := []struct {
		name     string
		args     map[string]any
		wantCode string
		wantText string
	}{
		{"empty repo_path", map[string]any{"repo_path": ""}, "invalid_argument", "empty"},
		{"whitespace root", map[string]any{"root": "   "}, "invalid_argument", "empty"},
		{"unexpanded env", map[string]any{"repo_path": "${CLAUDE_PROJECT_DIR}"}, "invalid_argument", "unexpanded"},
		{"relative path", map[string]any{"repo_path": "relative/repo"}, "invalid_argument", "relative"},
		{"missing dir", map[string]any{"repo_path": filepath.Join(repoA, "missing")}, "invalid_argument", "cannot be used"},
		{"not a dir", map[string]any{"root": notADir}, "invalid_argument", "not a directory"},
		{"disagreeing selectors", map[string]any{"repo_path": repoA, "root": repoB}, "invalid_argument", "disagree"},
		{"different repo without resolver", map[string]any{"repo_path": repoB}, "not_initialized", "cannot switch"},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			out, err := reg.Call(context.Background(), "devcouncil_next_task", tc.args)
			if err != nil {
				t.Fatal(err)
			}
			payload, ok := out.(devcouncil.ErrorPayload)
			if !ok {
				t.Fatalf("got %T %+v, want ErrorPayload", out, out)
			}
			if payload.OK || payload.Code != tc.wantCode || !strings.Contains(payload.Error, tc.wantText) {
				t.Fatalf("got %+v, want ok=false code=%q containing %q", payload, tc.wantCode, tc.wantText)
			}
		})
	}
}

func TestRegistryOpenStorePicksUpLaterInitializedStore(t *testing.T) {
	root := t.TempDir()
	dbPath := filepath.Join(root, ".devcouncil", "state.sqlite")
	dcstoreBin := testsupport.DCStore(t)

	reg := devcouncil.NewRegistry(root, nil, nil)
	reg.OpenStore = func() *store.Client {
		if _, err := os.Stat(dbPath); err != nil {
			return nil
		}
		c := store.New(dcstoreBin, dbPath)
		t.Cleanup(c.Close)
		return c
	}

	// Initially uninitialized
	out, err := reg.Call(context.Background(), "devcouncil_next_task", map[string]any{})
	if err != nil {
		t.Fatal(err)
	}
	if m := out.(map[string]any); m["code"] != "not_initialized" {
		t.Fatalf("before init = %+v, want not_initialized", m)
	}

	// Initialize store on disk without restarting registry
	if err := os.MkdirAll(filepath.Dir(dbPath), 0o755); err != nil {
		t.Fatal(err)
	}
	testsupport.SeedTask(t, dbPath, testsupport.TaskSeed{
		ID: "TASK-LATE", Title: "late", Status: "ready",
	})

	out, err = reg.Call(context.Background(), "devcouncil_next_task", map[string]any{})
	if err != nil {
		t.Fatal(err)
	}
	if m := out.(map[string]any); m["ok"] != true || m["task_id"] != "TASK-LATE" {
		t.Fatalf("after init = %+v, want TASK-LATE", m)
	}
}
