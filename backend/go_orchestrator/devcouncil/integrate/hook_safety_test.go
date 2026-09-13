package integrate

import (
	"bytes"
	"encoding/json"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
)

func TestHookCleanupPreservesSharedCursorConfig(t *testing.T) {
	root := t.TempDir()
	path := filepath.Join(root, ".cursor", "hooks.json")
	writeFile(t, path, `{"version":1,"custom":9007199254740993,"hooks":{"preToolUse":[{"command":"dev hook pre-tool-use"},{"command":"devmap hook session-start"}],"stop":[]}}`)
	_, err := Uninstall(UninstallOptions{Root: root, Mode: ModeApply})
	if err != nil {
		t.Fatal(err)
	}
	b, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("shared file lost: %v", err)
	}
	if !bytes.Contains(b, []byte("devmap hook")) || !bytes.Contains(b, []byte("9007199254740993")) || !bytes.Contains(b, []byte(`"stop"`)) {
		t.Fatalf("foreign data changed: %s", b)
	}
	if bytes.Contains(b, []byte("dev hook")) {
		t.Fatalf("legacy hook survived: %s", b)
	}
}

func TestHookCleanupCommandOwnership(t *testing.T) {
	for _, tc := range []struct {
		command string
		owned   bool
	}{
		{"dev hook user-prompt-submit", true},
		{"/Users/me/.local/bin/dev hook session-start", true},
		{`'/Users/a b/bin/dev' hook session-end --project-root '/a b'`, true},
		{`"C:\Program Files\devcouncil.exe" hook post-tool-use`, true},
		{"dev\t hook\tpost-tool-use", true},
		{"devmap hook session-start", false},
		{"mydev hook pre-tool-use", false},
		{"node other_dev_hook.js", false},
		{"echo 'dev hook user-prompt-submit'", false},
		{"dev hook session-end; other-hook", false},
		{"dev hooks session-start", false},
	} {
		t.Run(tc.command, func(t *testing.T) {
			var entry map[string]any
			if err := json.Unmarshal(mustJSON(map[string]string{"command": tc.command}), &entry); err != nil {
				t.Fatal(err)
			}
			if got := isDevCouncilHookEntry(entry); got != tc.owned {
				t.Fatalf("ownership=%v want %v", got, tc.owned)
			}
		})
	}
}

func TestHookCleanupCoversLegacyHosts(t *testing.T) {
	for _, rel := range []string{".codex/hooks.json", ".gemini/settings.json", ".grok/hooks/devcouncil.json"} {
		t.Run(rel, func(t *testing.T) {
			root := t.TempDir()
			path := filepath.Join(root, rel)
			writeFile(t, path, `{"permissions":{"allow":["Read"]},"hooks":{"BeforeTool":[{"hooks":[{"command":"dev hook pre-tool-use"},{"command":"other-hook"}]}]}}`)
			if _, err := Uninstall(UninstallOptions{Root: root, Mode: ModeApply}); err != nil {
				t.Fatal(err)
			}
			b, err := os.ReadFile(path)
			if err != nil {
				t.Fatal(err)
			}
			if bytes.Contains(b, []byte("dev hook")) || !bytes.Contains(b, []byte("other-hook")) || !bytes.Contains(b, []byte("permissions")) {
				t.Fatalf("bad cleanup: %s", b)
			}
		})
	}
}

func TestHookCleanupPreservesNumbersAndEmptyGroups(t *testing.T) {
	root := t.TempDir()
	path := filepath.Join(root, ".claude/settings.local.json")
	writeFile(t, path, `{"id":9007199254740993,"hooks":{"Stop":[{"hooks":[{"command":"dev hook agent-response"}]}],"Notification":[{"matcher":"keep","hooks":[]}]}}`)
	if _, err := Uninstall(UninstallOptions{Root: root}); err != nil {
		t.Fatal(err)
	}
	b, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Contains(b, []byte("9007199254740993")) || !bytes.Contains(b, []byte("Notification")) {
		t.Fatalf("unrelated values changed: %s", b)
	}
}

func TestHookCleanupPreflightsBeforeMutation(t *testing.T) {
	for _, body := range []string{`{"hooks":`, `{"hooks":{},"hooks":{}}`, `null`, strings.Repeat(" ", maxHostConfigBytes+1)} {
		root := t.TempDir()
		cursor := filepath.Join(root, ".cursor/hooks.json")
		original := `{"version":1,"hooks":{"stop":[{"command":"dev hook agent-response"}]}}`
		writeFile(t, cursor, original)
		writeFile(t, filepath.Join(root, ".claude/settings.local.json"), body)
		if _, err := Uninstall(UninstallOptions{Root: root}); err == nil {
			t.Fatalf("accepted unsafe JSON %.80q", body)
		}
		got, err := os.ReadFile(cursor)
		if err != nil || string(got) != original {
			t.Fatalf("partial mutation before validation: %s %v", got, err)
		}
	}
}

func TestHookCleanupRejectsSymlinks(t *testing.T) {
	for _, directory := range []bool{false, true} {
		t.Run(map[bool]string{true: "directory", false: "file"}[directory], func(t *testing.T) {
			root := t.TempDir()
			outside := t.TempDir()
			original := `{"hooks":{"Stop":[{"hooks":[{"command":"dev hook agent-response"}]}]}}`
			dest := filepath.Join(outside, "settings.json")
			writeFile(t, dest, original)
			if directory {
				if err := os.Symlink(outside, filepath.Join(root, ".claude")); err != nil {
					t.Fatal(err)
				}
			} else {
				if err := os.Mkdir(filepath.Join(root, ".claude"), 0700); err != nil {
					t.Fatal(err)
				}
				if err := os.Symlink(dest, filepath.Join(root, ".claude/settings.json")); err != nil {
					t.Fatal(err)
				}
			}
			if _, err := Uninstall(UninstallOptions{Root: root}); err == nil {
				t.Fatal("symlink was not reported as a refusal")
			}
			b, err := os.ReadFile(dest)
			if err != nil || string(b) != original {
				t.Fatal("outside data changed")
			}
		})
	}
}

func TestHookCleanupPreservesPermissions(t *testing.T) {
	root := t.TempDir()
	path := filepath.Join(root, ".claude/settings.local.json")
	writeFile(t, path, `{"hooks":{"Stop":[{"hooks":[{"command":"dev hook agent-response"}]}]}}`)
	if err := os.Chmod(path, 0600); err != nil {
		t.Fatal(err)
	}
	if _, err := Uninstall(UninstallOptions{Root: root}); err != nil {
		t.Fatal(err)
	}
	info, err := os.Stat(path)
	if err != nil {
		t.Fatal(err)
	}
	if info.Mode().Perm() != 0600 {
		t.Fatalf("permissions widened to %o", info.Mode().Perm())
	}
}

func TestHookCleanupRejectsInvalidMode(t *testing.T) {
	if _, err := Uninstall(UninstallOptions{Root: t.TempDir(), Mode: "aply"}); err == nil {
		t.Fatal("invalid mode silently accepted")
	}
}

func TestHookCleanupLeavesUnownedEmptyFiles(t *testing.T) {
	for _, body := range []string{`{"version":1}`, `{"version":1,"hooks":{"stop":[]}}`, `{}`} {
		root := t.TempDir()
		path := filepath.Join(root, ".cursor/hooks.json")
		writeFile(t, path, body)
		receipt, err := Uninstall(UninstallOptions{Root: root})
		if err != nil {
			t.Fatal(err)
		}
		b, err := os.ReadFile(path)
		if err != nil || string(b) != body {
			t.Fatalf("no ownership evidence yet file changed: %s %v", b, err)
		}
		if len(receipt.HookEntries) != 0 {
			t.Fatal("empty file counted as a managed hook")
		}
	}
}

func TestIntegrateRejectsRetiredWriteGateBeforeWrites(t *testing.T) {
	root := t.TempDir()
	_, err := Run(Options{Root: root, Host: "claude", Mode: ModeApply, WriteGate: true, DevmapBin: "/bin/true"})
	if err == nil {
		t.Fatal("unsupported write gate reported success")
	}
	entries, err := os.ReadDir(root)
	if err != nil {
		t.Fatal(err)
	}
	if len(entries) != 0 {
		t.Fatalf("wrote before rejecting flag: %v", entries)
	}
}

func TestHookCleanupBackupAndReadOnlyModes(t *testing.T) {
	root := t.TempDir()
	rel := ".claude/settings.local.json"
	path := filepath.Join(root, rel)
	original := `{"model":"keep","hooks":{"Stop":[{"hooks":[{"command":"dev hook agent-response"}]}]}}`
	writeFile(t, path, original)
	for _, mode := range []Mode{ModeCheck, ModeDryRun} {
		receipt, err := Uninstall(UninstallOptions{Root: root, Mode: mode})
		if err != nil {
			t.Fatal(err)
		}
		b, err := os.ReadFile(path)
		if err != nil || string(b) != original {
			t.Fatal("read-only changed settings")
		}
		if len(receipt.Backups) != 0 || receipt.HookEntries[rel] != 1 {
			t.Fatalf("bad preview: %#v", receipt)
		}
		entries, err := os.ReadDir(filepath.Dir(path))
		if err != nil || len(entries) != 1 {
			t.Fatalf("preview created files: %v %v", entries, err)
		}
	}
	receipt, err := Uninstall(UninstallOptions{Root: root})
	if err != nil {
		t.Fatal(err)
	}
	b, err := os.ReadFile(filepath.Join(root, receipt.Backups[rel]))
	if err != nil || string(b) != original {
		t.Fatalf("backup isn't exact: %s %v", b, err)
	}
	again, err := Uninstall(UninstallOptions{Root: root})
	if err != nil || len(again.Backups) != 0 || len(again.HookEntries) != 0 {
		t.Fatalf("not idempotent: %#v %v", again, err)
	}
}

func TestHookCleanupDetectsConcurrentEdits(t *testing.T) {
	rootPath := t.TempDir()
	rel := ".claude/settings.local.json"
	path := filepath.Join(rootPath, rel)
	writeFile(t, path, `{"original":true}`)
	root, err := os.OpenRoot(rootPath)
	if err != nil {
		t.Fatal(err)
	}
	defer func() {
		if err := root.Close(); err != nil {
			t.Error(err)
		}
	}()
	before, info, err := readHookFile(root, rel)
	if err != nil {
		t.Fatal(err)
	}
	newer := `{"another_writer":"keep this"}`
	writeFile(t, path, newer)
	if err := applyHookEdit(root, hookEdit{path: rel, before: before, after: []byte(`{}`), info: info}); err == nil {
		t.Fatal("overwrote concurrent edit")
	}
	b, err := os.ReadFile(path)
	if err != nil || string(b) != newer {
		t.Fatal("concurrent data lost")
	}
	entries, err := os.ReadDir(filepath.Dir(path))
	if err != nil || len(entries) != 1 {
		t.Fatalf("staging files leaked: %v %v", entries, err)
	}
}

func TestHookCleanupConcurrentWriters(t *testing.T) {
	root := t.TempDir()
	path := filepath.Join(root, ".claude/settings.local.json")
	original := `{"keep":"untouched","hooks":{"Stop":[{"hooks":[{"command":"dev hook agent-response"}]}]}}`
	writeFile(t, path, original)
	var wg sync.WaitGroup
	var successes atomic.Int32
	for i := 0; i < 32; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			_, err := Uninstall(UninstallOptions{Root: root})
			if err == nil {
				successes.Add(1)
			} else if !strings.Contains(err.Error(), "cleanup lock") {
				t.Error(err)
			}
		}()
	}
	wg.Wait()
	if successes.Load() == 0 {
		t.Fatal("no cleanup succeeded")
	}
	b, err := os.ReadFile(path)
	if err != nil || !bytes.Contains(b, []byte("untouched")) || bytes.Contains(b, []byte("dev hook")) {
		t.Fatalf("bad final state %s %v", b, err)
	}
	if _, err := os.Stat(filepath.Join(root, hookCleanupLock)); !os.IsNotExist(err) {
		t.Fatalf("lock leaked: %v", err)
	}
	backups, err := filepath.Glob(path + ".devcouncil-backup-*")
	if err != nil || len(backups) != 1 {
		t.Fatalf("expected one original backup: %v %v", backups, err)
	}
}

func TestHookCleanupOpenCodePreservesMCPAndForeignPlugin(t *testing.T) {
	root := t.TempDir()
	config := filepath.Join(root, "opencode.json")
	writeFile(t, config, `{"mcp":{"devcouncil":{"enabled":true}},"plugin":["./.devcouncil/integrations/opencode_devcouncil_plugin.mjs","other-plugin"]}`)
	plugin := filepath.Join(root, openCodeHookPlugin)
	writeFile(t, plugin, "export const DevCouncilOpenCodeHook = async () => {};\nspawnSync(\"devcouncil\", args);\n")
	receipt, err := Uninstall(UninstallOptions{Root: root, Client: "opencode"})
	if err != nil {
		t.Fatal(err)
	}
	b, err := os.ReadFile(config)
	if err != nil || !bytes.Contains(b, []byte("other-plugin")) || !bytes.Contains(b, []byte(`"mcp"`)) || bytes.Contains(b, []byte("opencode_devcouncil")) {
		t.Fatalf("bad config %s %v", b, err)
	}
	if receipt.Files[openCodeHookPlugin] != "removed" || len(receipt.Backups) != 2 {
		t.Fatalf("incomplete plugin migration: %#v", receipt)
	}
}

func TestHookCleanupPayloadBounds(t *testing.T) {
	for _, data := range []string{`{"hooks":{},"x":{"a":1,"a":2}}`, strings.Repeat("[", 66) + strings.Repeat("]", 66), `{} {}`, `[]`} {
		if _, _, err := cleanHookJSON([]byte(data), hookConfig{client: "claude"}); err == nil {
			t.Fatalf("accepted unsafe JSON %.80q", data)
		}
	}
	root := t.TempDir()
	path := filepath.Join(root, ".claude/settings.json")
	original := "{}" + strings.Repeat(" ", maxHostConfigBytes-2)
	writeFile(t, path, original)
	if _, err := Uninstall(UninstallOptions{Root: root, Mode: ModeCheck}); err != nil {
		t.Fatalf("exact bound refused: %v", err)
	}
}

func FuzzHookCleanupPreservesForeignData(f *testing.F) {
	for _, seed := range []string{"", "large-9007199254740993", "dev hook", "\x00", "'quoted'"} {
		f.Add(seed)
	}
	f.Fuzz(func(t *testing.T, foreign string) {
		foreign = strings.ToValidUTF8(foreign, "�")
		if len(foreign) > 10000 {
			t.Skip()
		}
		input := map[string]any{"foreign": foreign, "id": json.Number("9007199254740993"), "hooks": map[string]any{"Stop": []any{map[string]any{"hooks": []any{map[string]any{"command": "dev hook agent-response"}, map[string]any{"type": "http", "url": foreign}}}}}}
		raw, err := json.Marshal(input)
		if err != nil {
			t.Fatal(err)
		}
		cleaned, n, err := cleanHookJSON(raw, hookConfig{client: "claude"})
		if err != nil || n != 1 {
			t.Fatalf("cleanup failed %d %v", n, err)
		}
		var got map[string]any
		d := json.NewDecoder(bytes.NewReader(cleaned))
		d.UseNumber()
		if err := d.Decode(&got); err != nil {
			t.Fatal(err)
		}
		if got["foreign"] != foreign || got["id"] != json.Number("9007199254740993") {
			t.Fatalf("foreign fields changed: %#v", got)
		}
		again, n, err := cleanHookJSON(cleaned, hookConfig{client: "claude"})
		if err != nil || n != 0 || !bytes.Equal(cleaned, again) {
			t.Fatal("not idempotent")
		}
	})
}

func TestHookCleanupReportsUnrecognizedOwnedCommands(t *testing.T) {
	data := []byte(`{"hooks":{"Stop":[{"hooks":[{"name":"devcouncil-agent-response-ready","command":"env CUSTOM=1 dev hook agent-response"}]}]}}`)
	if _, _, err := cleanHookJSON(data, hookConfig{client: "claude"}); err == nil {
		t.Fatal("unrecognized managed hook reported clean")
	}
}

func TestHookCleanupOpenCodeAbsoluteRegistration(t *testing.T) {
	for _, fileURI := range []bool{false, true} {
		root := t.TempDir()
		plugin := filepath.Join(root, openCodeHookPlugin)
		ref := plugin
		if fileURI {
			ref = (&url.URL{Scheme: "file", Path: filepath.ToSlash(plugin)}).String()
		}
		raw, err := json.Marshal(map[string]any{"plugin": []string{ref, "keep"}})
		if err != nil {
			t.Fatal(err)
		}
		writeFile(t, filepath.Join(root, "opencode.json"), string(raw))
		writeFile(t, plugin, "export const DevCouncilOpenCodeHook = async () => {};\nspawnSync(\"devcouncil\", args);\n")
		if _, err := Uninstall(UninstallOptions{Root: root, Client: "opencode"}); err != nil {
			t.Fatal(err)
		}
		after, err := os.ReadFile(filepath.Join(root, "opencode.json"))
		if err != nil {
			t.Fatal(err)
		}
		if bytes.Contains(after, []byte("opencode_devcouncil_plugin")) {
			t.Fatalf("dangling plugin registration after deleting its file: %s", after)
		}
	}
}

func TestHookCleanupRefusesUninspectableShapes(t *testing.T) {
	for _, data := range []string{`{"hooks":null}`, `{"hooks":[]}`, `{"hooks":{"Stop":{}}}`, `{"hooks":{"Stop":[null]}}`, `{"hooks":{"Stop":[{"hooks":null}]}}`, `{"hooks":{"Stop":[{"hooks":["dev hook"]}]}}`} {
		if _, _, err := cleanHookJSON([]byte(data), hookConfig{client: "claude"}); err == nil {
			t.Fatalf("uninspectable shape reported clean: %s", data)
		}
	}
	for _, data := range []string{`{"plugin":{}}`, `{"plugin":[12]}`} {
		if _, _, err := cleanHookJSON([]byte(data), hookConfig{client: "opencode"}); err == nil {
			t.Fatalf("uninspectable plugin shape reported clean: %s", data)
		}
	}
}
