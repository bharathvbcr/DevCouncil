package integrate

import (
	"bytes"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
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
