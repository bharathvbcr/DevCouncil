package integrate

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// Host config filenames are repository content: `.cursor`, `.codex` and
// `.mcp.json` all arrive with a clone, and a tracked symlink is content too.
// Each test below wrote outside the repository, disclosed an outside file into
// it, or read without a bound before planWrite was rooted.

func TestIntegrateRefusesSymlinkedConfigParent(t *testing.T) {
	base := t.TempDir()
	root := filepath.Join(base, "repo")
	outside := filepath.Join(base, "outside")
	mustMkdirAll(t, root)
	mustMkdirAll(t, outside)
	if err := os.Symlink(outside, filepath.Join(root, ".cursor")); err != nil {
		t.Fatal(err)
	}
	repo := mustOpenRoot(t, root)

	receipt := &Receipt{Files: map[string]string{}}
	if err := integrateCursor(repo, root, "/bin/true", "/bin/true", ModeApply, receipt); err == nil {
		t.Fatalf("accepted a symlinked .cursor instead of refusing; receipt=%v", receipt.Files)
	}
	if _, err := os.Stat(filepath.Join(outside, "mcp.json")); err == nil {
		t.Fatalf("wrote outside the repository at %s", filepath.Join(outside, "mcp.json"))
	}
}

func TestIntegrateRefusesSymlinkedConfigLeaf(t *testing.T) {
	base := t.TempDir()
	root := filepath.Join(base, "repo")
	mustMkdirAll(t, root)
	secret := filepath.Join(base, "private.json")
	if err := os.WriteFile(secret, []byte(`{"apiKey":"sk-SUPER-SECRET-VALUE"}`), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(secret, filepath.Join(root, ".mcp.json")); err != nil {
		t.Fatal(err)
	}
	repo := mustOpenRoot(t, root)

	receipt := &Receipt{Files: map[string]string{}}
	if err := integrateClaude(repo, root, "/bin/true", "/bin/true", ModeApply, receipt); err == nil {
		t.Fatalf("wrote through a symlinked .mcp.json instead of refusing; receipt=%v", receipt.Files)
	}
	// Replacing the link with a regular file is how the outside file's
	// contents were copied into the repository, ready to be committed.
	info, err := os.Lstat(filepath.Join(root, ".mcp.json"))
	if err != nil {
		t.Fatal(err)
	}
	if info.Mode()&os.ModeSymlink == 0 {
		body, _ := os.ReadFile(filepath.Join(root, ".mcp.json"))
		if strings.Contains(string(body), "sk-SUPER-SECRET-VALUE") {
			t.Fatalf("linked private file disclosed into the repository:\n%s", body)
		}
		t.Fatal("replaced the tracked symlink with a regular file")
	}
}

func TestIntegrateBoundsHostConfigRead(t *testing.T) {
	root := t.TempDir()
	big := make([]byte, maxHostConfigBytes+(1<<20))
	for i := range big {
		big[i] = ' '
	}
	copy(big, []byte("{}"))
	if err := os.WriteFile(filepath.Join(root, ".mcp.json"), big, 0o644); err != nil {
		t.Fatal(err)
	}
	repo := mustOpenRoot(t, root)

	receipt := &Receipt{Files: map[string]string{}}
	if err := integrateClaude(repo, root, "/bin/true", "/bin/true", ModeCheck, receipt); err == nil {
		t.Fatalf("read a %d-byte host config with no bound (bound is %d); receipt=%v",
			len(big), maxHostConfigBytes, receipt.Files)
	}
}

// An ordinary repository still integrates: the containment must refuse links,
// not the supported case.
func TestIntegrateWritesOrdinaryRepository(t *testing.T) {
	root := t.TempDir()
	repo := mustOpenRoot(t, root)

	receipt := &Receipt{Files: map[string]string{}}
	if err := integrateCursor(repo, root, "/bin/true", "/bin/true", ModeApply, receipt); err != nil {
		t.Fatalf("integrateCursor: %v", err)
	}
	for _, rel := range []string{".cursor/mcp.json", ".cursor/rules/devcouncil.mdc"} {
		if receipt.Files[rel] != "wrote" {
			t.Fatalf("%s: receipt says %q, want \"wrote\"", rel, receipt.Files[rel])
		}
		info, err := os.Lstat(filepath.Join(root, filepath.FromSlash(rel)))
		if err != nil {
			t.Fatalf("%s: %v", rel, err)
		}
		if !info.Mode().IsRegular() {
			t.Fatalf("%s: not a regular file (%v)", rel, info.Mode())
		}
	}
	// No temporary is left behind by the atomic publish.
	entries, err := os.ReadDir(filepath.Join(root, ".cursor"))
	if err != nil {
		t.Fatal(err)
	}
	for _, e := range entries {
		if strings.Contains(e.Name(), ".devcouncil-tmp-") {
			t.Fatalf("left a temporary behind: %s", e.Name())
		}
	}
}

// An existing config is still merged, and the merge is not a link-follow.
func TestIntegrateMergesExistingRegularConfig(t *testing.T) {
	root := t.TempDir()
	prior := `{"mcpServers":{"other":{"command":"/bin/echo"}}}`
	if err := os.WriteFile(filepath.Join(root, ".mcp.json"), []byte(prior), 0o644); err != nil {
		t.Fatal(err)
	}
	repo := mustOpenRoot(t, root)

	receipt := &Receipt{Files: map[string]string{}}
	if err := integrateClaude(repo, root, "/bin/true", "/bin/true", ModeApply, receipt); err != nil {
		t.Fatalf("integrateClaude: %v", err)
	}
	body, err := os.ReadFile(filepath.Join(root, ".mcp.json"))
	if err != nil {
		t.Fatal(err)
	}
	for _, want := range []string{"other", "devcouncil", "devmap"} {
		if !strings.Contains(string(body), want) {
			t.Fatalf("merged config lost %q:\n%s", want, body)
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
