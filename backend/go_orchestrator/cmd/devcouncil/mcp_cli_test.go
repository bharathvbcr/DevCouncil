package main

import (
	"encoding/json"
	"io"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/gatescfg"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/mcp"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// TestDispatchMCPHelpDoesNotBlockOnStdin verifies that `devcouncil mcp --help`
// and `-h` print usage and exit 0 immediately instead of hanging waiting for
// JSON-RPC frames on stdin.
func TestDispatchMCPHelpDoesNotBlockOnStdin(t *testing.T) {
	// Replace os.Stdin with an open pipe that never writes or closes, so a
	// regression that enters Serve() would hang instead of returning immediately.
	pr, pw, err := os.Pipe()
	if err != nil {
		t.Fatal(err)
	}
	oldStdin := os.Stdin
	os.Stdin = pr
	t.Cleanup(func() {
		os.Stdin = oldStdin
		_ = pw.Close()
		_ = pr.Close()
	})

	for _, argv := range [][]string{
		{"mcp", "--help"},
		{"mcp", "-h"},
		{"mcp", "help"},
		{"mcp-server", "--help"},
	} {
		t.Run(strings.Join(argv, "_"), func(t *testing.T) {
			done := make(chan struct {
				code   int
				stderr string
			}, 1)
			go func() {
				stderr, restore := swapStderr(t)
				code := dispatch(argv)
				restore()
				done <- struct {
					code   int
					stderr string
				}{code, stderr.String()}
			}()
			select {
			case res := <-done:
				if res.code != 0 {
					t.Fatalf("%v exited %d, want 0; stderr=%q", argv, res.code, res.stderr)
				}
				if !strings.Contains(res.stderr, "devcouncil mcp") || !strings.Contains(res.stderr, "repo_path") {
					t.Fatalf("%v stderr missing mcp usage: %q", argv, res.stderr)
				}
			case <-time.After(2 * time.Second):
				t.Fatalf("%v hung waiting on stdin", argv)
			}
		})
	}
}

func TestDispatchMCPRejectsUnknownArguments(t *testing.T) {
	for _, argv := range [][]string{
		{"mcp", "bogus"},
		{"mcp", "--bogus"},
		{"mcp", "serve", "serve"},
		{"mcp", "--project-root", "relative/path"},
		{"mcp-server", "not-a-subcmd"},
	} {
		t.Run(strings.Join(argv, "_"), func(t *testing.T) {
			stderr, restore := swapStderr(t)
			code := dispatch(argv)
			restore()
			if code != 2 {
				t.Fatalf("%v exited %d, want 2; stderr=%q", argv, code, stderr.String())
			}
		})
	}
}

// TestMCPServerRoutesMultipleReposViaRepoPathAndRoot proves a single global MCP
// server can serve two distinct repositories using per-call repo_path / root.
func TestMCPServerRoutesMultipleReposViaRepoPathAndRoot(t *testing.T) {
	bin := testsupport.DCStore(t)
	t.Setenv("PATH", filepath.Dir(bin)+string(os.PathListSeparator)+os.Getenv("PATH"))

	makeRepo := func(taskID string) string {
		root := t.TempDir()
		if err := os.MkdirAll(filepath.Join(root, ".devcouncil"), 0o755); err != nil {
			t.Fatal(err)
		}
		testsupport.SeedTask(t, gatescfg.DefaultWorkbenchDB(root), testsupport.TaskSeed{
			ID: taskID, Title: "seed " + taskID, Status: "ready",
		})
		return root
	}
	repo1 := makeRepo("TASK-ONE")
	repo2 := makeRepo("TASK-TWO")

	pool := newMCPRegistryPool(io.Discard, "devcouncil mcp")
	defer pool.Close()

	st1 := openStore(repo1)
	if st1 != nil {
		defer st1.Close()
	}
	reg := newMCPRegistry(repo1, st1, io.Discard)
	reg.ResolveRepo = pool.Get

	pr, pw := io.Pipe()
	out := &lockedBuffer{}
	srv := &mcp.Server{Registry: reg, ResolveRegistry: pool.Get, In: pr, Out: out, Err: io.Discard}
	done := make(chan error, 1)
	go func() { done <- srv.Serve() }()
	defer func() {
		_ = pw.Close()
		<-done
	}()

	call := func(id int, args map[string]any) map[string]any {
		t.Helper()
		frame, _ := json.Marshal(map[string]any{
			"jsonrpc": "2.0",
			"id":      id,
			"method":  "tools/call",
			"params": map[string]any{
				"name":      "devcouncil_next_task",
				"arguments": args,
			},
		})
		if _, err := pw.Write(append(frame, '\n')); err != nil {
			t.Fatal(err)
		}
		deadline := time.Now().Add(15 * time.Second)
		for time.Now().Before(deadline) {
			for _, line := range out.lines() {
				var env struct {
					ID     int `json:"id"`
					Result struct {
						Content []struct {
							Text string `json:"text"`
						} `json:"content"`
					} `json:"result"`
				}
				if json.Unmarshal([]byte(line), &env) == nil && env.ID == id && len(env.Result.Content) > 0 {
					var payload map[string]any
					if err := json.Unmarshal([]byte(env.Result.Content[0].Text), &payload); err != nil {
						t.Fatalf("bad tool payload %q: %v", env.Result.Content[0].Text, err)
					}
					return payload
				}
			}
			time.Sleep(10 * time.Millisecond)
		}
		t.Fatalf("no reply for id=%d", id)
		return nil
	}

	if got := call(1, map[string]any{}); got["task_id"] != "TASK-ONE" {
		t.Fatalf("default root got %+v, want TASK-ONE", got)
	}
	if got := call(2, map[string]any{"repo_path": repo2}); got["task_id"] != "TASK-TWO" {
		t.Fatalf("repo_path=repo2 got %+v, want TASK-TWO", got)
	}
	if got := call(3, map[string]any{"root": repo1}); got["task_id"] != "TASK-ONE" {
		t.Fatalf("root=repo1 got %+v, want TASK-ONE", got)
	}
	if got := call(4, map[string]any{"root": repo2}); got["task_id"] != "TASK-TWO" {
		t.Fatalf("root=repo2 got %+v, want TASK-TWO", got)
	}
	_ = devcouncil.ServedToolNames()
}
