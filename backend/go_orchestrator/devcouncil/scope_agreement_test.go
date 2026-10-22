package devcouncil_test

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
)

// The preflight and the verifier answer one question — may this path change for
// this task — and they used to answer it from two models: policy lent a planned
// file's directory and subsystem, verification checked the exact planned set.
// For every path: a write the preflight allows without qualification must not
// become a blocking orphan, and a blocking orphan must have been refused or
// flagged by the preflight.
func TestPreflightAndVerifierAgreeOnWhichPathsMayChange(t *testing.T) {
	root := t.TempDir()
	reg := devcouncil.NewRegistry(root, scopedStore(t), enforcingGate(t, root))
	planned := []dc.PlannedFile{{Path: "src/a.go", AllowedChange: dc.ChangeModify}, {Path: ".env", AllowedChange: dc.ChangeModify}}

	for _, path := range []string{
		"src/a.go",        // planned
		"src/other.go",    // same directory
		"src/sub/deep.go", // same subsystem, deeper
		"docs/x.md",       // elsewhere
		"go.mod",          // root manifest
		"README.md",       // root file
		".devmap/repo_map.json",
		".devcouncil/state.sqlite",
		".git/hooks/pre-commit",
		"src/a_test.go", // new test beside the planned file
	} {
		for _, op := range []string{"create", "modify", "delete"} {
			args := map[string]any{"path": path, "task_id": "TASK-1", "operation": op}
			out := policyCheck(t, reg, args).(map[string]any)
			allowed, _ := out["allowed"].(bool)
			wouldBlock, _ := out["would_block_verification"].(bool)

			changed := []string{path}
			if verify.ToolOwned(path) {
				changed = nil // verification does not read tool state at all
			}
			blocking := false
			added := map[string]struct{}{}
			if op == "create" {
				added[path] = struct{}{}
			}
			for _, gap := range verify.DetectOrphanDiffGaps("TASK-1", planned, changed, added) {
				blocking = blocking || gap.Blocking
			}
			if allowed && !wouldBlock && blocking {
				t.Errorf("%s %s: the preflight allowed it with no warning and verification blocks it", op, path)
			}
			if blocking && allowed && !wouldBlock {
				t.Errorf("%s %s: blocking orphan not flagged", op, path)
			}
			if allowed && wouldBlock != blocking && !verify.ToolOwned(path) {
				t.Errorf("%s %s: would_block_verification=%v but verification's orphan blocking=%v", op, path, wouldBlock, blocking)
			}
		}
	}
}

// DevCouncil's own state is not the agent's work. In a fresh repository with no
// ignore rules, what the tools wrote — the task store and its sidecars, the
// config `gate set` writes, the correction manifest the previous verify wrote,
// the DevMap files — never reaches the changed set or the diff, so a task that
// touches only its planned file has nothing to be blamed for, on every run.
func TestToolOwnedStateIsNeverBlamed(t *testing.T) {
	root := t.TempDir()
	run := func(args ...string) {
		t.Helper()
		cmd := exec.Command("git", append([]string{"-C", root, "-c", "user.name=t", "-c", "user.email=t@t"}, args...)...)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v %s", args, err, out)
		}
	}
	write := func(rel, body string) {
		t.Helper()
		full := filepath.Join(root, rel)
		if err := os.MkdirAll(filepath.Dir(full), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(full, []byte(body), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	run("init", "-q")
	write("src/a.go", "package a\n")
	write(".devcouncil/tracked.sqlite", "v1")
	run("add", "-A")
	run("commit", "-q", "-m", "init")

	write("src/a.go", "package a\n\nvar X = 1\n")
	write(".devcouncil/tracked.sqlite", "v2 binary-ish \x00\x01")
	write(".devcouncil/state.sqlite", "\x00db")
	write(".devcouncil/state.sqlite-wal", "\x00wal")
	write(".devcouncil/state.sqlite-shm", "\x00shm")
	write(".devcouncil/config.yaml", "gates:\n  mode: enforce\n")
	write(".devcouncil/corrections/TASK-1-abc.json", "{}")
	write(".devmap/repo_map.json", "{}")
	write(".devmap/graph.sqlite", "\x00")

	var firstChanged []string
	for i := 0; i < 2; i++ {
		changed, diff, empty, err := verify.CollectGitState(context.Background(), root)
		if err != nil {
			t.Fatal(err)
		}
		if len(changed) != 1 || changed[0] != "src/a.go" {
			t.Fatalf("run %d: changed = %q, want only src/a.go", i, changed)
		}
		if empty {
			t.Fatalf("run %d: the planned edit vanished from the diff", i)
		}
		for _, banned := range []string{".devcouncil", ".devmap", "tracked.sqlite"} {
			if contains(diff, banned) {
				t.Errorf("run %d: the diff names %s:\n%s", i, banned, diff)
			}
		}
		if i == 0 {
			firstChanged = changed
		} else if len(firstChanged) != len(changed) {
			t.Errorf("run %d produced a different changed set than run 0", i)
		}
		// What verify wrote between the runs: a new manifest.
		write(".devcouncil/corrections/TASK-1-def.json", "{}")
	}
}

func contains(haystack, needle string) bool {
	for i := 0; i+len(needle) <= len(haystack); i++ {
		if haystack[i:i+len(needle)] == needle {
			return true
		}
	}
	return false
}
