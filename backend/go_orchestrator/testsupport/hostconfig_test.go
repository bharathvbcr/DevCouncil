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
// configuration; see HostConfigEnv's audit.
var nonConfigEnv = []string{
	"HOME", "PATH", "TMPDIR", "TERM", "NO_COLOR", "COLUMNS",
	"DEVMAP_HOME", "DEVMAP_NON_REFLINK_DIR", "RAYON_NUM_THREADS",
}

// A new environment variable in devmap's sources that names a home, config or
// directory must be classified: isolated in HostConfigEnv, or listed above as
// not a config location. Otherwise the isolation drifts from what devmap reads.
func TestHostConfigEnvCoversWhatDevmapReads(t *testing.T) {
	src := filepath.Join(RustWorkspace(t), "devmap-cli", "src")
	read := regexp.MustCompile(`env::var(?:_os)?\("([A-Z][A-Z0-9_]*)"`)
	configLike := regexp.MustCompile(`(HOME|CONFIG|_DIR)`)
	seen := map[string]string{}
	err := filepath.WalkDir(src, func(path string, d os.DirEntry, err error) error {
		if err != nil || d.IsDir() || !strings.HasSuffix(path, ".rs") {
			return err
		}
		body, err := os.ReadFile(path)
		if err != nil {
			return err
		}
		for _, m := range read.FindAllSubmatch(body, -1) {
			seen[string(m[1])] = path
		}
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	if _, ok := seen["HOME"]; !ok {
		t.Fatalf("found no HOME read under %s: the scan is not reading devmap's sources (saw %v)", src, seen)
	}
	if _, ok := seen["CLAUDE_CONFIG_DIR"]; !ok {
		t.Fatalf("devmap no longer reads CLAUDE_CONFIG_DIR; drop it from HostConfigEnv or fix the scan (saw %v)", seen)
	}
	for name, path := range seen {
		if configLike.MatchString(name) && !slices.Contains(HostConfigEnv, name) && !slices.Contains(nonConfigEnv, name) {
			t.Errorf("devmap reads %s (%s), which looks like a config location: add it to HostConfigEnv or to nonConfigEnv", name, path)
		}
	}
}
