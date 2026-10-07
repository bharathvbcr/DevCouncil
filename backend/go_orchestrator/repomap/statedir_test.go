package repomap

import (
	"os"
	"path/filepath"
	"testing"
)

// TestResolveStateDirPrecedence pins the four rungs of DevMap's rule
// (rust/devmap-extract/src/paths.rs, resolve_state_dir) so a drift between the
// two implementations fails here rather than as a gate reading a stale graph.
func TestResolveStateDirPrecedence(t *testing.T) {
	root := filepath.FromSlash("/repo")
	dirs := func(present ...string) func(string) bool {
		set := map[string]bool{}
		for _, p := range present {
			set[filepath.Join(root, p)] = true
		}
		return func(p string) bool { return set[p] }
	}
	abs := filepath.FromSlash("/elsewhere/state")
	cases := []struct {
		name  string
		home  string
		isDir func(string) bool
		want  string
	}{
		{"absolute home wins over both dirs", abs, dirs(".devmap", ".devcouncil"), abs},
		{"relative home joins the root", "custom", dirs(".devcouncil"), filepath.Join(root, "custom")},
		{"empty home is unset", "", dirs(".devcouncil"), filepath.Join(root, ".devcouncil")},
		{"standalone precedes legacy", "", dirs(".devmap", ".devcouncil"), filepath.Join(root, ".devmap")},
		{"legacy when only legacy exists", "", dirs(".devcouncil"), filepath.Join(root, ".devcouncil")},
		{"standalone when neither exists", "", dirs(), filepath.Join(root, ".devmap")},
	}
	for _, c := range cases {
		if got := ResolveStateDir(root, c.home, c.isDir); got != c.want {
			t.Errorf("%s: ResolveStateDir = %q, want %q", c.name, got, c.want)
		}
	}
}

// TestCodeGraphPathReadsTheFilesystem checks the wrapper consults real
// directories and the environment, not only the pure resolver.
func TestCodeGraphPathReadsTheFilesystem(t *testing.T) {
	t.Setenv(DevmapHomeEnv, "")
	root := t.TempDir()
	if err := os.Mkdir(filepath.Join(root, ".devcouncil"), 0o755); err != nil {
		t.Fatal(err)
	}
	want := filepath.Join(root, ".devcouncil", "graph", "code_graph.json")
	if got := CodeGraphPath(root); got != want {
		t.Fatalf("CodeGraphPath = %q, want %q", got, want)
	}
	// A file named .devmap is not a state directory.
	if err := os.WriteFile(filepath.Join(root, ".devmap"), nil, 0o644); err != nil {
		t.Fatal(err)
	}
	if got := CodeGraphPath(root); got != want {
		t.Fatalf("with a .devmap file: CodeGraphPath = %q, want %q", got, want)
	}
	home := t.TempDir()
	t.Setenv(DevmapHomeEnv, home)
	if got, want := CodeGraphPath(root), filepath.Join(home, "graph", "code_graph.json"); got != want {
		t.Fatalf("with DEVMAP_HOME: CodeGraphPath = %q, want %q", got, want)
	}
}
