package integrate

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// `.codex/config.toml` is the user's Codex configuration. `integrate codex
// --apply` replaced it with a one-line comment, deleting every
// `[mcp_servers.*]` table the user had, and registered nothing in exchange.
func TestApplyKeepsAnExistingCodexConfigByteForByte(t *testing.T) {
	root := t.TempDir()
	path := filepath.Join(root, ".codex", "config.toml")
	mustMkdirAll(t, filepath.Dir(path))
	prior := "model = \"o3\"\n\n[mcp_servers.mine]\ncommand = \"/opt/mine\"\nargs = [\"serve\"]\n"
	if err := os.WriteFile(path, []byte(prior), 0o644); err != nil {
		t.Fatal(err)
	}
	// A DevMap that does nothing: this test is about what the Go adapter
	// writes, and must not reach a real `devmap` on PATH or the user's home.
	_, runErr := Run(Options{Root: root, Host: "codex", Mode: ModeApply, DevmapBin: trueBinary(t)})
	got, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if string(got) != prior {
		t.Fatalf("apply rewrote the user's Codex config (err=%v):\n--- before\n%s--- after\n%s", runErr, prior, got)
	}
	// Kept intact by a refusal, which must then be a failure the caller sees.
	if runErr == nil {
		t.Fatal("apply reported success without registering anything in the existing config")
	}
}

// The class, not the case: replacing a file outright is only safe when the
// file on disk is one this command wrote. Anything else needs a merge.
func TestPlanWriteRefusesToReplaceAFileItDoesNotOwn(t *testing.T) {
	root := t.TempDir()
	rel := "notes/plain.txt"
	mustMkdirAll(t, filepath.Join(root, "notes"))
	prior := "someone else's file\n"
	if err := os.WriteFile(filepath.Join(root, rel), []byte(prior), 0o644); err != nil {
		t.Fatal(err)
	}
	receipt := &Receipt{Files: map[string]string{}}
	err := planWrite(mustOpenRoot(t, root), rel, []byte("ours\n"), ModeApply, receipt, nil)
	if err == nil {
		t.Fatalf("replaced an unowned file; receipt=%v", receipt.Files)
	}
	got, _ := os.ReadFile(filepath.Join(root, rel))
	if string(got) != prior {
		t.Fatalf("file changed despite the refusal: %q", got)
	}
	// Creating a file that is not there is still fine.
	fresh := "notes/new.txt"
	if err := planWrite(mustOpenRoot(t, root), fresh, []byte("ours\n"), ModeApply, receipt, nil); err != nil {
		t.Fatalf("refused to create a missing file: %v", err)
	}
}

// The Cursor rule is DevCouncil's file, so a rule this command wrote — in its
// current wording or an earlier one — is replaced. A user's own rule that
// happens to carry the name is not.
func TestCursorRuleReplacesOnlyARuleDevCouncilWrote(t *testing.T) {
	rel := ".cursor/rules/devcouncil.mdc"
	for name, tc := range map[string]struct {
		prior   string
		replace bool
	}{
		"current": {cursorRule, true},
		"earlier wording": {"---\ndescription: DevCouncil task loop and navigation\nalwaysApply: true\n---\n\n" +
			"# DevCouncil\n\nUse DevCouncil MCP tools for status.\n", true},
		"user's own rule": {"---\ndescription: my team's conventions\nalwaysApply: true\n---\n\n# Ours\n", false},
		"no frontmatter":  {"# DevCouncil\n\nhand-written\n", false},
	} {
		t.Run(name, func(t *testing.T) {
			root := t.TempDir()
			mustMkdirAll(t, filepath.Join(root, ".cursor", "rules"))
			if err := os.WriteFile(filepath.Join(root, rel), []byte(tc.prior), 0o644); err != nil {
				t.Fatal(err)
			}
			receipt := &Receipt{Files: map[string]string{}}
			err := writeCursorRule(mustOpenRoot(t, root), ModeApply, receipt)
			got, _ := os.ReadFile(filepath.Join(root, rel))
			if tc.replace {
				if err != nil {
					t.Fatalf("refused DevCouncil's own rule: %v", err)
				}
				if string(got) != cursorRule {
					t.Fatalf("rule not brought current:\n%s", got)
				}
				return
			}
			if err == nil || !strings.Contains(err.Error(), "did not write") {
				t.Fatalf("want a refusal naming the ownership problem, got %v", err)
			}
			if string(got) != tc.prior {
				t.Fatalf("user's rule changed: %q", got)
			}
		})
	}
}

func trueBinary(t *testing.T) string {
	t.Helper()
	for _, candidate := range []string{"/usr/bin/true", "/bin/true"} {
		if _, err := os.Stat(candidate); err == nil {
			return candidate
		}
	}
	t.Skip("no `true` binary to stand in for devmap")
	return ""
}
