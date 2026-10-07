package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// TestHookNoEventIsNoop confirms that `dev hook` with no event name exits 0.
func TestHookNoEventIsNoop(t *testing.T) {
	code := dispatch([]string{"hook"})
	if code != 0 {
		t.Fatalf("dispatch(hook) exit %d, want 0", code)
	}
}

// TestHookUserPromptSubmitIsNoop confirms the exact failing command from the
// bug report exits 0 instead of crashing with exit 2.
func TestHookUserPromptSubmitIsNoop(t *testing.T) {
	root := t.TempDir()
	code := dispatch([]string{
		"hook", "user-prompt-submit",
		"--client", "claude",
		"--project-root", root,
	})
	if code != 0 {
		t.Fatalf("user-prompt-submit exit %d, want 0", code)
	}
}

// TestHookAllRetiredEventsExit0 confirms every event that the legacy Python
// CLI handled exits 0. These are the events installed in
// .claude/settings.local.json by the old `integrate claude --apply`.
func TestHookAllRetiredEventsExit0(t *testing.T) {
	root := t.TempDir()
	events := []string{
		"user-prompt-submit",
		"agent-response",
		"stop-failure",
		"pre-compact",
		"post-compact",
		"subagent-start",
		"subagent-stop",
		"notification",
		"file-changed",
		"cwd-changed",
		"directory-added",
		"post-tool-batch",
		"claude-statusline",
	}
	for _, event := range events {
		t.Run(event, func(t *testing.T) {
			code := dispatch([]string{
				"hook", event,
				"--client", "claude",
				"--project-root", root,
			})
			if code != 0 {
				t.Fatalf("hook %s exit %d, want 0", event, code)
			}
		})
	}
}

// TestHookNeverExits2 asserts the hard invariant: no hook invocation may
// return exit code 2. Exit 2 means "block the agent" on Claude Code / Cursor.
func TestHookNeverExits2(t *testing.T) {
	root := t.TempDir()
	cases := [][]string{
		{"hook"},
		{"hook", "user-prompt-submit"},
		{"hook", "agent-response", "--client", "claude"},
		{"hook", "pre-compact", "--project-root", root},
		{"hook", "totally-unknown-event"},
		{"hook", ""},
		{"hook", "--client", "claude"},
		{"hook", "--defer-batch"},
		{"hook", "post-tool-use", "--unknown-future-flag", "value"},
	}
	for _, args := range cases {
		name := strings.Join(args, "_")
		t.Run(name, func(t *testing.T) {
			// devmap won't be found in a temp-dir PATH, so index events
			// degrade gracefully. Set environment to ensure no real binary.
			t.Setenv("DEVMAP_BIN", "")
			t.Setenv("HOME", t.TempDir())
			t.Setenv("PATH", t.TempDir())

			stderr, restore := swapStderr(t)
			code := dispatch(args)
			restore()
			if code == 2 {
				t.Fatalf("dispatch(%v) exit 2 (BLOCKS the agent), stderr=%s", args, stderr.String())
			}
		})
	}
}

// TestHookUnknownEventIsNoop ensures future hook events added by hosts
// degrade gracefully instead of crashing.
func TestHookUnknownEventIsNoop(t *testing.T) {
	code := dispatch([]string{"hook", "some-future-event-2027"})
	if code != 0 {
		t.Fatalf("unknown event exit %d, want 0", code)
	}
}

// TestHookAgentResponseIgnoresRetiredHookGateKey confirms the retired
// agent-response event is a silent no-op whatever a stale config says. Nothing
// reads execution.hook_gate.mode — not the CLI, not the MCP surface — so `off`,
// `contain` and an absent key exit 0 alike. These were two tests named
// "WithGateOff" and "WithGateContain", as if the key still chose a behaviour.
func TestHookAgentResponseIgnoresRetiredHookGateKey(t *testing.T) {
	for name, cfg := range map[string]string{
		"off":     "execution:\n  hook_gate:\n    mode: 'off'\n",
		"contain": "execution:\n  hook_gate:\n    mode: contain\n",
		"absent":  "gates:\n  mode: off\n",
	} {
		root := t.TempDir()
		cfgDir := filepath.Join(root, ".devcouncil")
		if err := os.MkdirAll(cfgDir, 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(cfgDir, "config.yaml"), []byte(cfg), 0o644); err != nil {
			t.Fatal(err)
		}
		code := dispatch([]string{
			"hook", "agent-response",
			"--client", "claude",
			"--project-root", root,
		})
		if code != 0 {
			t.Fatalf("hook_gate %s: agent-response exit %d, want 0", name, code)
		}
	}
}
