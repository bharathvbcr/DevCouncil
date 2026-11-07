package devmap

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// Adversarial-pass helper (not part of the lane). Modes:
//
//	stderr-hang: answers initialize, then on tools/call streams stderr forever and never answers.
//	ok:          answers everything; the CLI path answers `search`.
func TestWarmMCPStressHelperProcess(t *testing.T) {
	if os.Getenv("GO_WANT_WARM_ATTACK_HELPER") != "1" {
		return
	}
	mode := os.Getenv("WARM_ATTACK_MODE")
	var args []string
	for i, a := range os.Args {
		if a == "--" {
			args = os.Args[i+1:]
			break
		}
	}
	if len(args) >= 2 && args[len(args)-2] == "manifest" && args[len(args)-1] == "--help" {
		_, _ = os.Stdout.WriteString("Usage: devmap manifest --output <FILE> --graph-output <FILE>\n")
		os.Exit(0)
	}
	if len(args) == 0 || args[len(args)-1] != "mcp" {
		if len(args) >= 2 && args[1] == "search" {
			_, _ = os.Stdout.WriteString(`{"items":[{"file_path":"pkga/a.go","symbol_name":"Alpha","kind":"function","score":1.0,"source_span":"1..3"}],"hidden":0}` + "\n")
			os.Exit(0)
		}
		os.Exit(2)
	}
	scanner := bufio.NewScanner(os.Stdin)
	scanner.Buffer(make([]byte, 64<<10), 4<<20)
	write := func(v any) {
		b, _ := json.Marshal(v)
		_, _ = os.Stdout.Write(append(b, '\n'))
	}
	for scanner.Scan() {
		var req map[string]any
		if json.Unmarshal(scanner.Bytes(), &req) != nil {
			continue
		}
		id, hasID := req["id"]
		if !hasID {
			continue
		}
		switch req["method"] {
		case "initialize":
			write(map[string]any{"jsonrpc": "2.0", "id": id, "result": map[string]any{
				"protocolVersion": "2025-11-25",
				"capabilities":    map[string]any{"tools": map[string]any{}},
				"serverInfo":      map[string]any{"name": "devmap", "version": "0.1.0"},
			}})
		case "tools/call":
			if mode == "stderr-hang" {
				go func() {
					for {
						_, _ = os.Stderr.WriteString(strings.Repeat("x", 200) + "\n")
						time.Sleep(200 * time.Microsecond)
					}
				}()
				time.Sleep(time.Hour)
			}
			root := os.Getenv("WARM_ATTACK_ROOT")
			write(map[string]any{"jsonrpc": "2.0", "id": id, "result": map[string]any{
				"isError": false,
				"content": []map[string]any{{"type": "text", "text": "{}"}},
				"structuredContent": map[string]any{
					"items": []map[string]any{{"file_path": "pkga/a.go", "symbol_name": "Alpha", "kind": "function", "score": 1.0, "source_span": "1..4"}},
					"shown": 1, "hidden": 0, "total": 1, "truncated": false,
					"repository": map[string]any{"root": root, "store": filepath.Join(root, ".devcouncil/codeintel/devmap.sqlite"), "resolved_from": "repo_path"},
				},
			}})
		}
	}
}

func writeAttackWrapper(t *testing.T, mode, root string) string {
	t.Helper()
	wrapper := filepath.Join(t.TempDir(), "fake-devmap")
	script := fmt.Sprintf(
		"#!/bin/sh\nexport GO_WANT_WARM_ATTACK_HELPER=1\nexport WARM_ATTACK_MODE=%q\nexport WARM_ATTACK_ROOT=%q\nexec %q -test.run=^TestWarmMCPStressHelperProcess$ -- \"$@\"\n",
		mode, root, os.Args[0],
	)
	if err := os.WriteFile(wrapper, []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	return wrapper
}

// 18: a hung helper streaming stderr while the call times out. Run with -race.
func TestWarmMCPStderrTailIsRaceFreeOnTimeout(t *testing.T) {
	root := t.TempDir()
	c := NewWarm(writeAttackWrapper(t, "stderr-hang", root), root)
	c.Timeout = 600 * time.Millisecond
	defer c.Close()
	_, err := c.Search(WithCall(context.Background()), "Alpha")
	if err == nil {
		t.Fatal("expected a timeout")
	}
	t.Logf("err: %.200s", err)
}

// 21: cancel the owner context, then make a call. It must not fail with
// "context canceled"; it should fall back to the CLI.
func TestWarmMCPCallAfterOwnerCancelFallsBackToCLI(t *testing.T) {
	root := t.TempDir()
	owner, cancel := context.WithCancel(context.Background())
	c := New(writeAttackWrapper(t, "ok", root), root)
	c.EnableWarmMCPContext(owner)
	defer c.Close()
	if _, err := c.Search(WithCall(context.Background()), "Alpha"); err != nil {
		t.Fatalf("first call: %v", err)
	}
	cancel()
	time.Sleep(300 * time.Millisecond)
	found, err := c.Search(WithCall(context.Background()), "Alpha")
	if err != nil {
		t.Fatalf("call after owner cancel failed (no CLI fallback): %v", err)
	}
	if len(found.Items) != 1 {
		t.Fatalf("items: %+v", found.Items)
	}
}

// 10: EnableWarmMCPContext cancelled, then EnableWarmMCP: the dead owner stays.
func TestWarmMCPReenableAfterCancelInstallsTheNewOwner(t *testing.T) {
	root := t.TempDir()
	owner, cancel := context.WithCancel(context.Background())
	c := New(writeAttackWrapper(t, "ok", root), root)
	c.EnableWarmMCPContext(owner)
	defer c.Close()
	cancel()
	time.Sleep(300 * time.Millisecond)
	c.EnableWarmMCP()
	found, err := c.Search(WithCall(context.Background()), "Alpha")
	if err != nil {
		t.Fatalf("re-enabled client cannot serve: %v", err)
	}
	if len(found.Items) != 1 {
		t.Fatalf("items: %+v", found.Items)
	}
}
