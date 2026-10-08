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
// It now gains DevCouncil's server, and every byte that was there survives.
func TestApplyKeepsAnExistingCodexConfigByteForByte(t *testing.T) {
	bin := devmapForTest(t)
	root := t.TempDir()
	path := filepath.Join(root, ".codex", "config.toml")
	mustMkdirAll(t, filepath.Dir(path))
	prior := "# mine\nmodel = \"o3\"\n\n[mcp_servers.mine]\ncommand = \"/opt/mine\"\nargs = [\"serve\"]\n"
	if err := os.WriteFile(path, []byte(prior), 0o644); err != nil {
		t.Fatal(err)
	}
	receipt := apply(t, bin, root, "codex")
	got, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.HasPrefix(string(got), prior) {
		t.Fatalf("apply rewrote the user's Codex config:\n--- before\n%s--- after\n%s", prior, got)
	}
	if !strings.Contains(string(got), "[mcp_servers.devcouncil]") || !strings.Contains(string(got), fakeSelf) {
		t.Fatalf("no DevCouncil server registered:\n%s", got)
	}
	if receipt.Files[".codex/config.toml"] != "wrote" {
		t.Fatalf("receipt does not record the write: %v", receipt.Files)
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
