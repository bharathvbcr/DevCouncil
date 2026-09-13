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

// TestHookAgentResponseWithGateOff confirms agent-response is a no-op when
// hook_gate.mode is off (the default).
func TestHookAgentResponseWithGateOff(t *testing.T) {
	root := t.TempDir()
	// Write a config with hook_gate: off
	cfgDir := filepath.Join(root, ".devcouncil")
	if err := os.MkdirAll(cfgDir, 0o755); err != nil {
		t.Fatal(err)
	}
	cfg := "execution:\n  hook_gate:\n    mode: 'off'\n"
	if err := os.WriteFile(filepath.Join(cfgDir, "config.yaml"), []byte(cfg), 0o644); err != nil {
		t.Fatal(err)
	}

	code := dispatch([]string{
		"hook", "agent-response",
		"--client", "claude",
		"--project-root", root,
	})
	if code != 0 {
		t.Fatalf("agent-response with gate off exit %d, want 0", code)
	}
}

// TestHookAgentResponseWithGateContain confirms agent-response exits 0 even
// when containment is on — the CLI path no longer blocks; the MCP surface
// enforces the actual gate.
func TestHookAgentResponseWithGateContain(t *testing.T) {
	root := t.TempDir()
	cfgDir := filepath.Join(root, ".devcouncil")
	if err := os.MkdirAll(cfgDir, 0o755); err != nil {
		t.Fatal(err)
	}
	cfg := "execution:\n  hook_gate:\n    mode: contain\n"
	if err := os.WriteFile(filepath.Join(cfgDir, "config.yaml"), []byte(cfg), 0o644); err != nil {
		t.Fatal(err)
	}

	code := dispatch([]string{
		"hook", "agent-response",
		"--client", "claude",
		"--project-root", root,
	})
	if code != 0 {
		t.Fatalf("agent-response with gate contain exit %d, want 0", code)
	}
}
