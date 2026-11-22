package testsupport

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// HostConfigEnv is every environment variable that decides where a real devmap
// (or a host CLI it configures) finds the developer's user-level configuration.
// IsolateHostConfig points HOME at scratch and unsets the rest, so a test that drives the real binary
// cannot reach the developer's own files, and so the answer to "what does a
// test have to isolate?" lives here rather than in each test's comment.
//
// Audit of rust/devmap-cli/src (rg 'env::var(_os)?\("'), read for the paths
// `devmap integrate`, `install` and `doctor` write or read:
//
//   - HOME: ~/.cursor/mcp.json, ~/.codex/config.toml, ~/.claude.json, and the
//     fallback for the Claude config directory (integrate.rs, claude.rs,
//     installation.rs, hook.rs, commands/doctor.rs).
//   - CLAUDE_CONFIG_DIR: when set and non-empty it replaces ~/.claude and moves
//     the global config to $CLAUDE_CONFIG_DIR/.claude.json (claude.rs
//     claude_config_dir / claude_global_config). A seat-pinned session sets it
//     to the seat's real profile.
//   - CODEX_HOME: not read by devmap today (integrate.rs writes
//     ~/.codex/config.toml from HOME). It is redirected anyway because the
//     Codex CLI honours it, so a future devmap that does would still be
//     contained.
//
// Not config locations, so not isolated: PATH and TMPDIR (binary and scratch
// discovery), TERM/NO_COLOR/COLUMNS (presentation), DEVMAP_HOME (the index
// state directory), DEVMAP_NON_REFLINK_DIR and RAYON_NUM_THREADS (tuning).
// XDG_CONFIG_HOME is read only by git, in devcouncil/verify, whose tests
// isolate it themselves.
//
// Adding a variable devmap honours for a user-level config means adding it
// here; TestHostConfigEnvCoversWhatDevmapReads fails when the Rust source reads
// a config-directory style variable this list does not name.
var HostConfigEnv = []string{"CLAUDE_CONFIG_DIR", "CODEX_HOME"}

// HostConfig is the scratch user-level configuration a test runs against.
type HostConfig struct {
	// Home is the scratch $HOME.
	Home string
}

// IsolateHostConfig points HOME at a fresh scratch directory and neutralises
// every override in HostConfigEnv for the rest of the test.
//
// CLAUDE_CONFIG_DIR is removed rather than redirected, so devmap resolves the
// Claude configuration the way an unpinned session does, under the scratch
// HOME (~/.claude.json, ~/.claude): the paths the host tests already assert.
// CODEX_HOME is removed for the same reason. The variables are restored when
// the test ends.
func IsolateHostConfig(t testing.TB) HostConfig {
	t.Helper()
	home := t.TempDir()
	t.Setenv("HOME", home)
	for _, name := range HostConfigEnv {
		// t.Setenv registers the restore; Unsetenv then removes the variable
		// itself, since devmap treats an empty value as unset but other
		// readers might not.
		t.Setenv(name, "")
		if err := os.Unsetenv(name); err != nil {
			t.Fatalf("unset %s: %v", name, err)
		}
	}
	return HostConfig{Home: home}
}

// HostConfigChildEnv is the environment for a child process a test launches
// with an explicit Env: the process environment with HOME set to home and every
// HostConfigEnv variable removed. Building Env from os.Environ() with only HOME
// overridden hands the child the developer's CLAUDE_CONFIG_DIR.
func HostConfigChildEnv(home string) []string {
	drop := map[string]bool{"HOME": true}
	for _, name := range HostConfigEnv {
		drop[name] = true
	}
	var env []string
	for _, kv := range os.Environ() {
		name, _, _ := strings.Cut(kv, "=")
		if !drop[name] {
			env = append(env, kv)
		}
	}
	return append(env, "HOME="+home)
}

// AssertWithin fails the test for every path that does not lie under one of
// roots. A relative path is taken as relative to the first root, which is where
// a receipt that names repository files puts them. It is the guard that no
// receipt names a file outside the scratch directories the test owns: a path
// outside them is the developer's own configuration.
func AssertWithin(t testing.TB, roots []string, paths ...string) {
	t.Helper()
	if len(roots) == 0 {
		t.Fatal("AssertWithin needs at least one scratch root")
	}
	var abs []string
	for _, root := range roots {
		abs = append(abs, resolveForCompare(root))
	}
	for _, path := range paths {
		if !filepath.IsAbs(path) {
			path = filepath.Join(roots[0], path)
		}
		path = resolveForCompare(path)
		inside := false
		for _, root := range abs {
			if path == root || strings.HasPrefix(path, root+string(filepath.Separator)) {
				inside = true
				break
			}
		}
		if !inside {
			t.Errorf("%s lies outside the test's scratch roots %s", path, fmt.Sprint(abs))
		}
	}
}

// resolveForCompare cleans a path and resolves symlinks in its longest
// existing prefix, so /var and /private/var (macOS temp directories) compare
// equal while a path that does not exist yet still compares.
func resolveForCompare(path string) string {
	path = filepath.Clean(path)
	rest := ""
	for p := path; ; p = filepath.Dir(p) {
		if resolved, err := filepath.EvalSymlinks(p); err == nil {
			return filepath.Join(resolved, rest)
		}
		parent := filepath.Dir(p)
		if parent == p {
			return path
		}
		rest = filepath.Join(filepath.Base(p), rest)
	}
}
