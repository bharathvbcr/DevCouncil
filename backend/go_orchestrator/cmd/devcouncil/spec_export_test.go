package main

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
)

func TestSpecExportTasksPersistsWorkbenchItemsWithScopeAndDAG(t *testing.T) {
	root := t.TempDir()
	specDir := filepath.Join(root, "specs", "002-zero-copy-worktrees")
	if err := os.MkdirAll(specDir, 0o755); err != nil {
		t.Fatal(err)
	}
	longProse := "Implement bidirectional `same_registered_repo`, `--root` worktree support in `mcp::open_for_call`, asymmetric `RootResolveInput::resolve`, `PostToolUse` lazy seed in `hook.rs`, and guaranteed `--seed-from` manifest write across all stacked worktrees and warm MCP servers with strict zero-write-on-read invariants and atomic staging"
	tasksMd := "# Tasks: 002 - Zero-Copy\n## Phase 1: Core\n" +
		"- [x] [T-001] [US1] " + longProse + " `rust/devmap-extract/src/paths.rs` `rust/devmap-serve/src/mcp.rs` tests: cargo test -p devmap-serve\n" +
		"- [ ] [T-002] [P] [US2] Retire legacy overlay sidecars `src-tauri/src/codeintel/worktree_overlay.rs` depends_on: [T-001] tests: cargo test --lib worktree_overlay\n"
	if err := os.WriteFile(filepath.Join(specDir, "tasks.md"), []byte(tasksMd), 0o644); err != nil {
		t.Fatal(err)
	}

	stdout, restoreOut := swapStdout(t)
	stderr, restoreErr := swapStderr(t)
	code := dispatch([]string{"spec", "export-tasks", "002-zero-copy-worktrees", "--json", "--project-root", root})
	restoreOut()
	restoreErr()
	if code != 0 {
		t.Fatalf("spec export-tasks exit %d stderr=%s stdout=%s", code, stderr.String(), stdout.String())
	}

	// Verify `devcouncil task show T-001 --json --project-root <root>` succeeds against `.devcouncil/state.sqlite`!
	showOut, restoreShowOut := swapStdout(t)
	showErr, restoreShowErr := swapStderr(t)
	showCode := dispatch([]string{"task", "show", "T-001", "--json", "--project-root", root})
	restoreShowOut()
	restoreShowErr()
	if showCode != 0 {
		t.Fatalf("task show T-001 exit %d stderr=%s stdout=%s", showCode, showErr.String(), showOut.String())
	}

	var payload struct {
		OK   bool `json:"ok"`
		Item struct {
			ID    string `json:"id"`
			Links []struct {
				Kind   string `json:"kind"`
				ItemID string `json:"item_id"`
			} `json:"links"`
			Scope struct {
				PlannedFiles []any `json:"planned_files"`
			} `json:"scope"`
		} `json:"item"`
	}
	if err := json.Unmarshal(showOut.Bytes(), &payload); err != nil {
		t.Fatalf("unmarshal task show output: %v (%s)", err, showOut.String())
	}
	if !payload.OK || payload.Item.ID != "T-001" {
		t.Fatalf("unexpected task show payload: %s", showOut.String())
	}
	if len(payload.Item.Scope.PlannedFiles) != 2 {
		t.Fatalf("expected 2 planned_files on T-001, got %v", payload.Item.Scope.PlannedFiles)
	}
	if len(payload.Item.Links) != 1 || payload.Item.Links[0].Kind != "blocks" || payload.Item.Links[0].ItemID != "T-002" {
		t.Fatalf("expected T-001 to carry blocks link to T-002, got %+v", payload.Item.Links)
	}

	// Re-running `spec export-tasks` must be idempotent!
	out2, rOut2 := swapStdout(t)
	err2, rErr2 := swapStderr(t)
	code2 := dispatch([]string{"spec", "export-tasks", "002-zero-copy-worktrees", "--json", "--project-root", root})
	rOut2()
	rErr2()
	if code2 != 0 {
		t.Fatalf("idempotent re-export exit %d stderr=%s stdout=%s", code2, err2.String(), out2.String())
	}
}
