package integrate

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// The Cursor rule is the one file this package still writes, and its path is
// repository content: `.cursor` arrives with a clone, and a tracked symlink is
// content too. Each test below wrote outside the repository, disclosed an
// outside file into it, or read without a bound before planWrite was rooted.
// The host documents DevMap writes carry the same contracts in
// `rust/devmap-cli/src/integrate.rs`
// (`a_linked_document_or_parent_is_refused_and_nothing_lands_outside`).

func TestCursorRuleRefusesSymlinkedConfigParent(t *testing.T) {
	base := t.TempDir()
	root := filepath.Join(base, "repo")
	outside := filepath.Join(base, "outside")
	mustMkdirAll(t, root)
	mustMkdirAll(t, outside)
	if err := os.Symlink(outside, filepath.Join(root, ".cursor")); err != nil {
		t.Fatal(err)
	}
	receipt := &Receipt{Files: map[string]string{}}
	if err := writeCursorRule(mustOpenRoot(t, root), ModeApply, receipt); err == nil {
		t.Fatalf("accepted a symlinked .cursor instead of refusing; receipt=%v", receipt.Files)
	}
	if entries, _ := os.ReadDir(outside); len(entries) != 0 {
		t.Fatalf("wrote outside the repository: %v", entries)
	}
}

func TestCursorRuleRefusesSymlinkedLeaf(t *testing.T) {
	base := t.TempDir()
	root := filepath.Join(base, "repo")
	mustMkdirAll(t, filepath.Join(root, ".cursor", "rules"))
	secret := filepath.Join(base, "private.mdc")
	if err := os.WriteFile(secret, []byte("sk-SUPER-SECRET-VALUE\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	leaf := filepath.Join(root, filepath.FromSlash(cursorRuleRel))
	if err := os.Symlink(secret, leaf); err != nil {
		t.Fatal(err)
	}
	receipt := &Receipt{Files: map[string]string{}}
	if err := writeCursorRule(mustOpenRoot(t, root), ModeApply, receipt); err == nil {
		t.Fatalf("wrote through a symlinked rule instead of refusing; receipt=%v", receipt.Files)
	}
	info, err := os.Lstat(leaf)
	if err != nil {
		t.Fatal(err)
	}
	if info.Mode()&os.ModeSymlink == 0 {
		t.Fatal("replaced the tracked symlink with a regular file")
	}
	if body, _ := os.ReadFile(secret); string(body) != "sk-SUPER-SECRET-VALUE\n" {
		t.Fatalf("the linked file changed: %q", body)
	}
}

func TestPlanWriteBoundsTheRead(t *testing.T) {
	root := t.TempDir()
	mustMkdirAll(t, filepath.Join(root, ".cursor", "rules"))
	big := []byte(cursorRule + strings.Repeat(" ", maxHostConfigBytes+(1<<20)))
	if err := os.WriteFile(filepath.Join(root, filepath.FromSlash(cursorRuleRel)), big, 0o644); err != nil {
		t.Fatal(err)
	}
	receipt := &Receipt{Files: map[string]string{}}
	if err := writeCursorRule(mustOpenRoot(t, root), ModeCheck, receipt); err == nil {
		t.Fatalf("read an oversized file without a bound; receipt=%v", receipt.Files)
	}
}

// An ordinary repository still integrates: the containment must refuse links,
// not the supported case.
func TestCursorRuleWritesOrdinaryRepository(t *testing.T) {
	root := t.TempDir()
	receipt := &Receipt{Files: map[string]string{}}
	if err := writeCursorRule(mustOpenRoot(t, root), ModeApply, receipt); err != nil {
		t.Fatalf("writeCursorRule: %v", err)
	}
	if receipt.Files[cursorRuleRel] != "wrote" {
		t.Fatalf("receipt says %q, want \"wrote\"", receipt.Files[cursorRuleRel])
	}
	info, err := os.Lstat(filepath.Join(root, filepath.FromSlash(cursorRuleRel)))
	if err != nil || !info.Mode().IsRegular() {
		t.Fatalf("not a regular file: %v %v", info, err)
	}
	entries, err := os.ReadDir(filepath.Join(root, ".cursor", "rules"))
	if err != nil {
		t.Fatal(err)
	}
	for _, e := range entries {
		if strings.Contains(e.Name(), ".devcouncil-tmp-") {
			t.Fatalf("left a temporary behind: %s", e.Name())
		}
	}
}

func mustMkdirAll(t *testing.T, p string) {
	t.Helper()
	if err := os.MkdirAll(p, 0o755); err != nil {
		t.Fatal(err)
	}
}

func mustOpenRoot(t *testing.T, p string) *os.Root {
	t.Helper()
	root, err := os.OpenRoot(p)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = root.Close() })
	return root
}
