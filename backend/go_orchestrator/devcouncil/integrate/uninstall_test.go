package integrate

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
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
	writeFile(t, cursorHooks, `{"version":1,"hooks":{"sessionStart":[{"command":"dev hook session-start"}]}}`)
	writeFile(t, grokHooks, `{"hooks":{"Stop":[{"hooks":[{"command":"dev hook agent-response"}]}]}}`)

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
	writeFile(t, cursorHooks, `{"version":1,"hooks":{"stop":[{"command":"dev hook agent-response"}]}}`)

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

// integrate cursor --apply must not put a DevCouncil hook back. DevMap
// writes its own marker-owned `.cursor/hooks.json`; nothing in it may be a
// DevCouncil lifecycle hook, which are retired.
func TestIntegrateCursorWritesNoDevCouncilHook(t *testing.T) {
	bin := devmapForTest(t)
	root := t.TempDir()
	receipt := apply(t, bin, root, "cursor")
	if receipt.Files[".cursor/mcp.json"] != "wrote" {
		t.Fatalf("mcp.json not written: %#v", receipt.Files)
	}
	body, err := os.ReadFile(filepath.Join(root, ".cursor", "hooks.json"))
	if os.IsNotExist(err) {
		return
	}
	if err != nil {
		t.Fatal(err)
	}
	for _, retired := range []string{"devcouncil hook", "dev hook", fakeSelf} {
		if strings.Contains(string(body), retired) {
			t.Fatalf(".cursor/hooks.json carries a DevCouncil hook (%q):\n%s", retired, body)
		}
	}
}

func TestUninstallPreservesKeyOrderEscapingAndFormatting(t *testing.T) {
	t.Run("claude nested matcher and statusLine", func(t *testing.T) {
		root := t.TempDir()
		settings := filepath.Join(root, ".claude", "settings.json")
		writeFile(t, settings, `{
    "z_first": "keep-first",
    "statusLine": {"type": "command", "command": "dev hook status-line"},
    "hooks": {
        "UserPromptSubmit": [
            {"matcher": "x", "hooks": [
                {"type": "command", "command": "dev hook user-prompt-submit"},
                {"type": "command", "command": "echo hi && cat foo > bar"}
            ]}
        ],
        "PreToolUse": [
            {"matcher": "", "hooks": [{"type": "command", "command": "dev hook pre-tool-use"}]}
        ]
    },
    "a_second": 42
}
`)
		if _, err := Uninstall(UninstallOptions{Root: root, Client: "claude", Target: TargetHooks, Mode: ModeApply}); err != nil {
			t.Fatalf("uninstall: %v", err)
		}
		gotBytes, err := os.ReadFile(settings)
		if err != nil {
			t.Fatal(err)
		}
		got := string(gotBytes)
		want := `{
    "z_first": "keep-first",
    "hooks": {
        "UserPromptSubmit": [
            {"matcher": "x", "hooks": [
                {"type": "command", "command": "echo hi && cat foo > bar"}
            ]}
        ]
    },
    "a_second": 42
}
`
		if got != want {
			t.Fatalf("claude mismatch:\nGOT:\n%s\nWANT:\n%s", got, want)
		}
	})

	t.Run("cursor flat hooks and compact formatting", func(t *testing.T) {
		root := t.TempDir()
		cursorHooks := filepath.Join(root, ".cursor", "hooks.json")
		writeFile(t, cursorHooks, `{"z_custom":"val","version":1,"hooks":{"stop":[{"command":"dev hook agent-response"},{"command":"echo ok && date > /dev/null"}],"beforeFileEdit":[{"command":"lint && format > out"}]},"a_tail":true}`)

		if _, err := Uninstall(UninstallOptions{Root: root, Client: "cursor", Target: TargetHooks, Mode: ModeApply}); err != nil {
			t.Fatalf("uninstall: %v", err)
		}
		gotBytes, err := os.ReadFile(cursorHooks)
		if err != nil {
			t.Fatal(err)
		}
		got := string(gotBytes)
		want := `{"z_custom":"val","version":1,"hooks":{"stop":[{"command":"echo ok && date > /dev/null"}],"beforeFileEdit":[{"command":"lint && format > out"}]},"a_tail":true}`
		if got != want {
			t.Fatalf("cursor mismatch:\nGOT:\n%s\nWANT:\n%s", got, want)
		}
	})

	t.Run("opencode plugin array and scalar", func(t *testing.T) {
		root := t.TempDir()
		openCode := filepath.Join(root, "opencode.json")
		writeFile(t, openCode, `{
    "z_theme": "solarized",
    "plugin": [
        "`+openCodeHookPlugin+`",
        "other-plugin && check > log"
    ],
    "a_other": {"nested": [1, 2]}
}`)
		if _, err := Uninstall(UninstallOptions{Root: root, Client: "opencode", Target: TargetHooks, Mode: ModeApply}); err != nil {
			t.Fatalf("uninstall: %v", err)
		}
		gotBytes, err := os.ReadFile(openCode)
		if err != nil {
			t.Fatal(err)
		}
		got := string(gotBytes)
		want := `{
    "z_theme": "solarized",
    "plugin": [
        "other-plugin && check > log"
    ],
    "a_other": {"nested": [1, 2]}
}`
		if got != want {
			t.Fatalf("opencode array mismatch:\nGOT:\n%s\nWANT:\n%s", got, want)
		}

		// Scalar string plugin key is removed without disturbing surviving keys.
		writeFile(t, openCode, `{"z_first":"hi > out && ok","plugin":"`+openCodeHookPlugin+`","a_last":1}`)
		if _, err := Uninstall(UninstallOptions{Root: root, Client: "opencode", Target: TargetHooks, Mode: ModeApply}); err != nil {
			t.Fatalf("uninstall scalar: %v", err)
		}
		gotBytes, err = os.ReadFile(openCode)
		if err != nil {
			t.Fatal(err)
		}
		got = string(gotBytes)
		want = `{"z_first":"hi > out && ok","a_last":1}`
		if got != want {
			t.Fatalf("opencode scalar mismatch:\nGOT:\n%s\nWANT:\n%s", got, want)
		}
	})
}
