package repomap

import (
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// ResolveStateDir is a port of devmap's rule, kept in step by this test rather
// than by a comment: for every rung of the precedence the real binary is asked
// where it keeps state (`devmap paths --json`) and this package must answer the
// same. A change to the rule in rust/devmap-extract/src/paths.rs that is not
// carried here fails this test instead of pointing the write gate at a graph
// the producer no longer writes.
func TestStateDirAgreesWithDevmapOnEveryRung(t *testing.T) {
	bin := testsupport.Devmap(t)

	mk := func(t *testing.T, dirs ...string) string {
		t.Helper()
		root := t.TempDir()
		for _, d := range dirs {
			if err := os.MkdirAll(filepath.Join(root, d), 0o755); err != nil {
				t.Fatal(err)
			}
		}
		resolved, err := filepath.EvalSymlinks(root)
		if err != nil {
			t.Fatal(err)
		}
		return resolved
	}
	for name, tc := range map[string]struct {
		dirs []string
		home string // relative homes are joined onto the root below
	}{
		"neither directory":       {},
		"only .devmap":            {dirs: []string{".devmap"}},
		"only .devcouncil":        {dirs: []string{".devcouncil"}},
		"both: .devmap wins":      {dirs: []string{".devmap", ".devcouncil"}},
		"relative DEVMAP_HOME":    {dirs: []string{".devcouncil"}, home: "custom-state"},
		"absolute DEVMAP_HOME":    {home: "@abs"},
		"DEVMAP_HOME beats both":  {dirs: []string{".devmap", ".devcouncil"}, home: "custom-state"},
		"empty DEVMAP_HOME unset": {dirs: []string{".devcouncil"}, home: ""},
	} {
		t.Run(name, func(t *testing.T) {
			root := mk(t, tc.dirs...)
			home := tc.home
			if home == "@abs" {
				home = filepath.Join(mk(t), "elsewhere")
			}
			t.Setenv(DevmapHomeEnv, home)

			cmd := exec.Command(bin, "paths", "--json", "--root", root)
			cmd.Env = append(os.Environ(), DevmapHomeEnv+"="+home)
			out, err := cmd.Output()
			if err != nil {
				t.Fatalf("devmap paths: %v", err)
			}
			var paths struct {
				StateDir  string `json:"state_dir"`
				CodeGraph string `json:"code_graph"`
			}
			if err := json.Unmarshal(out, &paths); err != nil {
				t.Fatal(err)
			}
			if got := StateDir(root); got != paths.StateDir {
				t.Errorf("StateDir = %s, devmap says %s", got, paths.StateDir)
			}
			if got := CodeGraphPath(root); got != paths.CodeGraph {
				t.Errorf("CodeGraphPath = %s, devmap says %s", got, paths.CodeGraph)
			}
		})
	}
}
