package integrate

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// The `devmap` server entry belongs to `devmap integrate <host>`, which pins it
// with `--root` and, for Claude, withholds it beside the enabled Dev Map
// plugin. This adapter used to write an unpinned copy first, so every apply
// undid DevMap's entry and every check reported drift against it.
func TestHostAdaptersLeaveTheDevmapEntryToDevmap(t *testing.T) {
	adapters := map[string]struct {
		rel string
		run func(repo *os.Root, root string, mode Mode, receipt *Receipt) error
	}{
		"claude": {".mcp.json", func(repo *os.Root, root string, mode Mode, receipt *Receipt) error {
			return integrateClaude(repo, root, "/bin/true", mode, receipt)
		}},
		"cursor": {".cursor/mcp.json", func(repo *os.Root, root string, mode Mode, receipt *Receipt) error {
			return integrateCursor(repo, root, "/bin/true", mode, receipt)
		}},
	}
	for host, adapter := range adapters {
		t.Run(host+"/fresh", func(t *testing.T) {
			root := t.TempDir()
			receipt := &Receipt{Files: map[string]string{}}
			if err := adapter.run(mustOpenRoot(t, root), root, ModeApply, receipt); err != nil {
				t.Fatal(err)
			}
			servers := mcpServers(t, filepath.Join(root, adapter.rel))
			if _, ok := servers["devcouncil"]; !ok {
				t.Fatalf("devcouncil entry missing: %v", servers)
			}
			if entry, ok := servers["devmap"]; ok {
				t.Fatalf("wrote a devmap entry DevMap owns: %v", entry)
			}
		})
		t.Run(host+"/devmap-pinned-it", func(t *testing.T) {
			root := t.TempDir()
			path := filepath.Join(root, adapter.rel)
			if err := adapter.run(mustOpenRoot(t, root), root, ModeApply,
				&Receipt{Files: map[string]string{}}); err != nil {
				t.Fatal(err)
			}
			// What `devmap integrate` leaves after it: the same file with its
			// own pinned entry beside ours.
			servers := mcpServers(t, path)
			servers["devmap"] = map[string]any{
				"type": "stdio", "command": "/opt/devmap", "args": []string{"--root", root, "mcp"},
			}
			if err := os.WriteFile(path, mustJSON(map[string]any{"mcpServers": servers}), 0o644); err != nil {
				t.Fatal(err)
			}
			check := &Receipt{Files: map[string]string{}}
			if err := adapter.run(mustOpenRoot(t, root), root, ModeCheck, check); err != nil {
				t.Fatal(err)
			}
			if got := check.Files[adapter.rel]; got != "unchanged" {
				t.Fatalf("check reports %q for a file only DevMap's entry differs in", got)
			}
			if err := adapter.run(mustOpenRoot(t, root), root, ModeApply,
				&Receipt{Files: map[string]string{}}); err != nil {
				t.Fatal(err)
			}
			args, _ := mcpServers(t, path)["devmap"].(map[string]any)["args"].([]any)
			if len(args) == 0 || args[0] != "--root" {
				t.Fatalf("apply rewrote DevMap's pinned entry: %v", args)
			}
		})
	}
}

// A check runs nothing, so it must say it did not look at DevMap's half
// rather than return a receipt that reads as a clean installation.
func TestCheckSaysDevmapAssetsWereNotExamined(t *testing.T) {
	receipt, err := Run(Options{Root: t.TempDir(), Host: "claude", Mode: ModeCheck})
	if err != nil {
		t.Fatal(err)
	}
	if len(receipt.Spawned) != 0 {
		t.Fatalf("check spawned %v", receipt.Spawned)
	}
	for _, note := range receipt.Notes {
		if strings.Contains(note, "not examined") && strings.Contains(note, "devmap integrate claude --check") {
			return
		}
	}
	t.Fatalf("no note says devmap assets were not examined: %q", receipt.Notes)
}

func mcpServers(t *testing.T, path string) map[string]any {
	t.Helper()
	body, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var doc map[string]any
	if err := json.Unmarshal(body, &doc); err != nil {
		t.Fatalf("%s: %v", path, err)
	}
	servers, ok := doc["mcpServers"].(map[string]any)
	if !ok {
		t.Fatalf("%s: no mcpServers object", path)
	}
	return servers
}
