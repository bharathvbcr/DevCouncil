package testsupport

import (
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strings"
	"testing"
)

func TestIsolateHostConfigRemovesEveryOverride(t *testing.T) {
	realSeat := t.TempDir()
	for _, name := range HostConfigEnv {
		t.Setenv(name, realSeat)
	}
	realHome := os.Getenv("HOME")

	t.Run("isolated", func(t *testing.T) {
		cfg := IsolateHostConfig(t)
		if cfg.Home == "" || cfg.Home == realHome || os.Getenv("HOME") != cfg.Home {
			t.Fatalf("HOME not redirected: cfg %q, env %q, was %q", cfg.Home, os.Getenv("HOME"), realHome)
		}
		for _, name := range HostConfigEnv {
			if v, set := os.LookupEnv(name); set {
				t.Errorf("%s is still set to %q", name, v)
			}
		}
	})

	if os.Getenv("HOME") != realHome {
		t.Errorf("HOME not restored: %q", os.Getenv("HOME"))
	}
	for _, name := range HostConfigEnv {
		if os.Getenv(name) != realSeat {
			t.Errorf("%s not restored: %q", name, os.Getenv(name))
		}
	}
}

func TestAssertWithin(t *testing.T) {
	scratch := t.TempDir()
	other := t.TempDir()
	AssertWithin(t, []string{scratch}, "rel/file.json", filepath.Join(scratch, "a", "b"), scratch)

	sub := &recorder{TB: t}
	AssertWithin(sub, []string{scratch}, filepath.Join(other, ".claude.json"), scratch+"-sibling/x")
	if len(sub.errs) != 2 {
		t.Fatalf("want both outside paths reported, got %v", sub.errs)
	}
}

// recorder captures Errorf so the guard's failures can be asserted on.
type recorder struct {
	testing.TB
	errs []string
}

func (r *recorder) Errorf(format string, args ...any) { r.errs = append(r.errs, format) }

// nonConfigEnv are the variables devmap reads that do not locate user-level
// configuration; see HostConfigEnv's audit. HOME is not here: it does locate
// configuration and is handled by IsolateHostConfig itself.
var nonConfigEnv = []string{
	"PATH", "TMPDIR", "TERM", "NO_COLOR", "COLUMNS",
	"DEVMAP_HOME", "RAYON_NUM_THREADS",
	// Project-root hints: repository-scoped, not user-level.
	"CLAUDE_PROJECT_DIR", "CURSOR_PROJECT_DIR",
}

// devmapCrateSources returns the src directory of devmap-cli and of every crate
// it depends on through path dependencies, transitively.
func devmapCrateSources(t *testing.T) []string {
	t.Helper()
	workspace := RustWorkspace(t)
	// Only dependency tables carry "name = { path = ... }"; [[bin]] and [[test]]
	// entries also have a path key, to a source file.
	pathDep := regexp.MustCompile(`(?m)^[A-Za-z0-9_-]+\s*=\s*\{[^}]*\bpath\s*=\s*"([^"]+)"`)
	var srcs []string
	seen := map[string]bool{}
	queue := []string{filepath.Join(workspace, "devmap-cli")}
	for len(queue) > 0 {
		dir := queue[0]
		queue = queue[1:]
		dir = filepath.Clean(dir)
		if seen[dir] {
			continue
		}
		seen[dir] = true
		manifest, err := os.ReadFile(filepath.Join(dir, "Cargo.toml"))
		if err != nil {
			t.Fatalf("read the manifest of a crate devmap-cli depends on: %v", err)
		}
		srcs = append(srcs, filepath.Join(dir, "src"))
		for _, m := range pathDep.FindAllSubmatch(manifest, -1) {
			queue = append(queue, filepath.Join(dir, string(m[1])))
		}
	}
	if len(srcs) < 5 {
		t.Fatalf("followed path dependencies to only %v: the walk is not reading devmap-cli's manifest", srcs)
	}
	return srcs
}

// Every upper-case string literal in the sources of devmap-cli and its path
// dependencies that looks like a home, config or directory variable must be
// classified: isolated (HostConfigEnv, or HOME), or listed in nonConfigEnv as
// not a config location. Literals are matched wherever they appear, so a read
// through a constant (env::var(CONST)) is caught as well as env::var("NAME").
// An allowlist entry no source mentions any more fails too, so it cannot go
// stale.
func TestHostConfigEnvCoversWhatDevmapReads(t *testing.T) {
	literal := regexp.MustCompile(`"([A-Z][A-Z0-9_]*)"`)
	configLike := regexp.MustCompile(`^[A-Z0-9_]*(HOME|CONFIG|_DIR)(_|$)`)
	seen := map[string]string{}
	for _, src := range devmapCrateSources(t) {
		err := filepath.WalkDir(src, func(path string, d os.DirEntry, err error) error {
			if err != nil || d.IsDir() || !strings.HasSuffix(path, ".rs") {
				return err
			}
			// Test code sets variables for git children (GIT_CONFIG_GLOBAL and
			// friends, which devmap itself never reads) and reads test-only ones
			// (DEVMAP_NON_REFLINK_DIR, CARGO_MANIFEST_DIR); the shipped binary is
			// what has to be isolated.
			if name := d.Name(); name == "tests.rs" || strings.HasSuffix(name, "_tests.rs") {
				return nil
			}
			body, err := os.ReadFile(path)
			if err != nil {
				return err
			}
			if cut := strings.Index(string(body), "#[cfg(test)]"); cut >= 0 {
				body = body[:cut] // by convention the test module closes the file
			}
			for _, m := range literal.FindAllSubmatch(body, -1) {
				seen[string(m[1])] = path
			}
			return nil
		})
		if err != nil {
			t.Fatal(err)
		}
	}
	for _, must := range []string{"HOME", "CLAUDE_CONFIG_DIR"} {
		if _, ok := seen[must]; !ok {
			t.Fatalf("devmap's sources no longer mention %s; fix the scan or HostConfigEnv (saw %d literals)", must, len(seen))
		}
	}
	for name, path := range seen {
		classified := name == "HOME" || slices.Contains(HostConfigEnv, name) || slices.Contains(nonConfigEnv, name)
		if configLike.MatchString(name) && !classified {
			t.Errorf("devmap's sources mention %s (%s), which looks like a config location: add it to HostConfigEnv or to nonConfigEnv", name, path)
		}
	}
	for _, name := range nonConfigEnv {
		if _, ok := seen[name]; !ok {
			t.Errorf("nonConfigEnv lists %s but no devmap source mentions it: remove the stale entry", name)
		}
	}
}

func TestHostConfigChildEnv(t *testing.T) {
	t.Setenv("CLAUDE_CONFIG_DIR", "/real/seat")
	t.Setenv("CODEX_HOME", "/real/codex")
	t.Setenv("HOME", "/real/home")
	t.Setenv("KEEP_ME", "1")
	env := HostConfigChildEnv("/scratch")
	got := map[string]string{}
	for _, kv := range env {
		k, v, _ := strings.Cut(kv, "=")
		got[k] = v
	}
	if got["HOME"] != "/scratch" || got["KEEP_ME"] != "1" {
		t.Errorf("HOME/KEEP_ME wrong: %v", got["HOME"])
	}
	for _, name := range HostConfigEnv {
		if _, ok := got[name]; ok {
			t.Errorf("%s leaked into the child environment", name)
		}
	}
}
