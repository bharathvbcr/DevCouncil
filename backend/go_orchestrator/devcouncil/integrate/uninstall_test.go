package integrate

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
)

func writeFile(t *testing.T, path, body string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
}

func TestUninstallHooksRemovesOwnedFiles(t *testing.T) {
	root := t.TempDir()
	cursorHooks := filepath.Join(root, ".cursor", "hooks.json")
	grokHooks := filepath.Join(root, ".grok", "hooks", "devcouncil.json")
	writeFile(t, cursorHooks, `{"version":1,"hooks":{"sessionStart":[]}}`)
	writeFile(t, grokHooks, `{"hooks":{}}`)

	receipt, err := Uninstall(UninstallOptions{Root: root, Target: TargetHooks, Mode: ModeApply})
	if err != nil {
		t.Fatalf("uninstall: %v", err)
	}
	for _, path := range []string{cursorHooks, grokHooks} {
		if _, err := os.Stat(path); !os.IsNotExist(err) {
			t.Fatalf("%s still present (stat err %v)", path, err)
		}
	}
	if got := receipt.Files[".cursor/hooks.json"]; got != "removed" {
		t.Fatalf("cursor receipt = %q, want removed", got)
	}
	if got := receipt.Files[".grok/hooks/devcouncil.json"]; got != "removed" {
		t.Fatalf("grok receipt = %q, want removed", got)
	}
}

func TestUninstallHooksDryRunRemovesNothing(t *testing.T) {
	root := t.TempDir()
	cursorHooks := filepath.Join(root, ".cursor", "hooks.json")
	writeFile(t, cursorHooks, `{"version":1}`)

	receipt, err := Uninstall(UninstallOptions{Root: root, Target: TargetHooks, Mode: ModeDryRun})
	if err != nil {
		t.Fatalf("uninstall: %v", err)
	}
	if _, err := os.Stat(cursorHooks); err != nil {
		t.Fatalf("dry run removed the file: %v", err)
	}
	if got := receipt.Files[".cursor/hooks.json"]; got != "would_remove" {
		t.Fatalf("receipt = %q, want would_remove", got)
	}
}

// A host settings file is shared with other tools, so uninstall must take out
// DevCouncil's entries and leave everything else exactly as it was.
func TestUninstallHooksStripsOnlyDevCouncilEntries(t *testing.T) {
	root := t.TempDir()
	settings := filepath.Join(root, ".claude", "settings.json")
	writeFile(t, settings, `{
  "model": "opus",
  "hooks": {
    "UserPromptSubmit": [
      {"matcher": "", "hooks": [
        {"type": "command", "command": "dev hook user-prompt-submit"},
        {"type": "command", "command": "/Users/me/.kanban-code/hook.sh"}
      ]}
    ],
    "PreToolUse": [
      {"matcher": "", "hooks": [{"type": "command", "command": "dev hook pre-tool-use"}]}
    ],
    "Stop": [
      {"matcher": "", "hooks": [{"type": "http", "url": "http://127.0.0.1:1/stop"}]}
    ]
  }
}`)

	receipt, err := Uninstall(UninstallOptions{Root: root, Target: TargetHooks, Mode: ModeApply})
	if err != nil {
		t.Fatalf("uninstall: %v", err)
	}
	if got := receipt.Files[".claude/settings.json"]; got != "cleaned" {
		t.Fatalf("receipt = %q, want cleaned", got)
	}

	raw, err := os.ReadFile(settings)
	if err != nil {
		t.Fatal(err)
	}
	var out map[string]any
	if err := json.Unmarshal(raw, &out); err != nil {
		t.Fatalf("rewritten settings do not parse: %v", err)
	}
	if out["model"] != "opus" {
		t.Fatalf("unrelated key lost: %#v", out["model"])
	}
	hooks, ok := out["hooks"].(map[string]any)
	if !ok {
		t.Fatalf("hooks key lost while a foreign hook remained: %#v", out)
	}
	// PreToolUse held only DevCouncil's entry, so the whole event goes.
	if _, present := hooks["PreToolUse"]; present {
		t.Fatalf("PreToolUse survived with nothing in it: %#v", hooks)
	}
	// UserPromptSubmit keeps the foreign command and loses DevCouncil's.
	ups, ok := hooks["UserPromptSubmit"].([]any)
	if !ok || len(ups) != 1 {
		t.Fatalf("UserPromptSubmit = %#v", hooks["UserPromptSubmit"])
	}
	group := ups[0].(map[string]any)
	entries := group["hooks"].([]any)
	if len(entries) != 1 {
		t.Fatalf("expected 1 surviving entry, got %#v", entries)
	}
	if cmd := entries[0].(map[string]any)["command"]; cmd != "/Users/me/.kanban-code/hook.sh" {
		t.Fatalf("wrong entry survived: %#v", cmd)
	}
	if _, present := hooks["Stop"]; !present {
		t.Fatalf("foreign Stop hook removed: %#v", hooks)
	}
}

func TestUninstallHooksDropsHooksKeyWhenItEmpties(t *testing.T) {
	root := t.TempDir()
	settings := filepath.Join(root, ".claude", "settings.local.json")
	writeFile(t, settings, `{"hooks":{"Stop":[{"matcher":"","hooks":[{"type":"command","command":"devcouncil hook agent-response"}]}]}}`)

	if _, err := Uninstall(UninstallOptions{Root: root, Target: TargetHooks, Mode: ModeApply}); err != nil {
		t.Fatalf("uninstall: %v", err)
	}
	raw, err := os.ReadFile(settings)
	if err != nil {
		t.Fatal(err)
	}
	var out map[string]any
	if err := json.Unmarshal(raw, &out); err != nil {
		t.Fatal(err)
	}
	if _, present := out["hooks"]; present {
		t.Fatalf("empty hooks key kept: %s", raw)
	}
}

func TestUninstallHooksIsIdempotentOnACleanTree(t *testing.T) {
	root := t.TempDir()
	receipt, err := Uninstall(UninstallOptions{Root: root, Target: TargetHooks, Mode: ModeApply})
	if err != nil {
		t.Fatalf("uninstall: %v", err)
	}
	for rel, action := range receipt.Files {
		if action != "missing" {
			t.Fatalf("%s = %q on an empty tree, want missing", rel, action)
		}
	}
}

func TestUninstallRefusesUnknownTarget(t *testing.T) {
	if _, err := Uninstall(UninstallOptions{Root: t.TempDir(), Target: "everything"}); err == nil {
		t.Fatal("expected an error for an unknown target")
	}
}

// A settings file we cannot parse is a file we must not rewrite: silently
// dropping the reader's hooks is worse than refusing.
func TestUninstallRefusesUnparsableSettings(t *testing.T) {
	root := t.TempDir()
	settings := filepath.Join(root, ".claude", "settings.json")
	writeFile(t, settings, `{"hooks": {`)

	if _, err := Uninstall(UninstallOptions{Root: root, Target: TargetHooks, Mode: ModeApply}); err == nil {
		t.Fatal("expected a refusal on unparsable JSON")
	}
	raw, err := os.ReadFile(settings)
	if err != nil {
		t.Fatal(err)
	}
	if string(raw) != `{"hooks": {` {
		t.Fatalf("file was rewritten: %s", raw)
	}
}

// integrate cursor --apply must not put a hook config back.
func TestIntegrateCursorWritesNoHooksFile(t *testing.T) {
	root := t.TempDir()
	receipt := &Receipt{Host: "cursor", Mode: string(ModeApply), Files: map[string]string{}}
	if err := integrateCursor(root, "/bin/true", "/bin/true", ModeApply, false, receipt); err != nil {
		t.Fatalf("integrateCursor: %v", err)
	}
	if _, err := os.Stat(filepath.Join(root, ".cursor", "hooks.json")); !os.IsNotExist(err) {
		t.Fatalf("integrate wrote .cursor/hooks.json (stat err %v)", err)
	}
	if _, present := receipt.Files[".cursor/hooks.json"]; present {
		t.Fatalf("receipt still claims a hooks file: %#v", receipt.Files)
	}
	if receipt.Files[".cursor/mcp.json"] != "wrote" {
		t.Fatalf("mcp.json not written: %#v", receipt.Files)
	}
}
