package gatescfg

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestNormalizeEmptyIsOff(t *testing.T) {
	mode, origin := Normalize("")
	if mode != ModeOff || origin != OriginDefault {
		t.Fatalf("Normalize(%q) = %s/%s, want off/default", "", mode, origin)
	}
}

func TestNormalizeFalseYAMLBoolIsOff(t *testing.T) {
	mode, origin := Normalize("false")
	if mode != ModeOff || origin != OriginConfig {
		t.Fatalf("got %s/%s", mode, origin)
	}
}

func TestNormalizeEnforceAliases(t *testing.T) {
	for _, raw := range []string{"enforce", "true", "1", "yes"} {
		mode, origin := Normalize(raw)
		if mode != ModeEnforce || origin != OriginConfig {
			t.Fatalf("Normalize(%q) = %s/%s", raw, mode, origin)
		}
	}
}

func TestParseModeRefusesUnknown(t *testing.T) {
	if _, err := ParseMode("yolo"); err == nil {
		t.Fatal("expected error")
	}
}

func TestLoadMissingFileDefaultsOff(t *testing.T) {
	snap := Load(t.TempDir())
	if snap.VerificationMode != ModeOff || snap.ConfigPresent {
		t.Fatalf("%+v", snap)
	}
}

func TestLoadReadsNestedGatesMode(t *testing.T) {
	root := t.TempDir()
	dir := filepath.Join(root, ".devcouncil")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	body := "gates:\n  mode: false\nexecution:\n  hook_gate:\n    mode: off\n"
	if err := os.WriteFile(filepath.Join(dir, "config.yaml"), []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
	snap := Load(root)
	if snap.VerificationMode != ModeOff {
		t.Fatalf("mode=%q want off (YAML false)", snap.VerificationMode)
	}
	if !snap.ConfigPresent {
		t.Fatal("expected config present")
	}
}

// A config created by `gate set` holds the one key this package reads. It used
// to also carry execution.hook_gate.mode, retired with the lifecycle hooks.
func TestSetVerificationModeCreatesOnlyLiveKeys(t *testing.T) {
	root := t.TempDir()
	path := filepath.Join(root, ".devcouncil", "config.yaml")
	if err := SetVerificationMode(path, "advisory"); err != nil {
		t.Fatal(err)
	}
	got, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if want := "gates:\n  mode: advisory\n"; string(got) != want {
		t.Fatalf("new config = %q, want %q", got, want)
	}
	if snap := Load(root); snap.VerificationMode != ModeAdvisory || snap.VerificationOrigin != OriginConfig {
		t.Fatalf("Load of the new config = %s/%s, want advisory/config",
			snap.VerificationMode, snap.VerificationOrigin)
	}
}

func TestSetVerificationModePatchesExisting(t *testing.T) {
	path := filepath.Join(t.TempDir(), "config.yaml")
	orig := "project:\n  name: demo\ngates:\n  mode: enforce\nindexing:\n  auto_refresh: true\n"
	if err := os.WriteFile(path, []byte(orig), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := SetVerificationMode(path, "off"); err != nil {
		t.Fatal(err)
	}
	got, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(got), "mode: off") {
		t.Fatalf("patched file:\n%s", got)
	}
	if !strings.Contains(string(got), "name: demo") || !strings.Contains(string(got), "auto_refresh: true") {
		t.Fatalf("surgery rewrote unrelated keys:\n%s", got)
	}
}

// D8 (2026-10-09): an unrecognised mode is a bad config, and a bad config
// fails closed. The earlier expectation, off/invalid, let a typo such as
// `enforced` switch the gate off without a word.
func TestNormalizeUnknownIsEnforceInvalid(t *testing.T) {
	mode, origin := Normalize("yolo")
	if mode != ModeEnforce || origin != OriginInvalid {
		t.Fatalf("Normalize(yolo) = %s/%s, want enforce/invalid", mode, origin)
	}
}

func TestLoadBrokenYAMLIsPresentInvalid(t *testing.T) {
	root := t.TempDir()
	dir := filepath.Join(root, ".devcouncil")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "config.yaml"), []byte("gates: [\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	snap := Load(root)
	if snap.VerificationMode != ModeEnforce {
		t.Fatalf("broken YAML must fail closed: %+v", snap)
	}
	if snap.Error == "" {
		t.Fatalf("broken YAML must carry its error: %+v", snap)
	}
	if !snap.ConfigPresent {
		t.Fatal("broken file must not look like a missing file")
	}
	if snap.VerificationOrigin != OriginInvalid {
		t.Fatalf("origin=%q want invalid", snap.VerificationOrigin)
	}
}

func TestSetVerificationModePatchesCRLF(t *testing.T) {
	path := filepath.Join(t.TempDir(), "config.yaml")
	orig := "project:\r\n  name: demo\r\ngates:\r\n  mode: enforce\r\n"
	if err := os.WriteFile(path, []byte(orig), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := SetVerificationMode(path, "off"); err != nil {
		t.Fatal(err)
	}
	got, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(got), "mode: off") {
		t.Fatalf("CRLF patch failed:\n%s", got)
	}
	if strings.Contains(string(got), "mode: enforce") {
		t.Fatalf("old mode survived:\n%s", got)
	}
}

func TestSetVerificationModeAtomicNoTmpLeft(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "config.yaml")
	if err := os.WriteFile(path, []byte("gates:\n  mode: enforce\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := SetVerificationMode(path, "advisory"); err != nil {
		t.Fatal(err)
	}
	entries, err := os.ReadDir(dir)
	if err != nil {
		t.Fatal(err)
	}
	for _, e := range entries {
		if strings.HasSuffix(e.Name(), ".tmp") {
			t.Fatalf("left temp file %s", e.Name())
		}
	}
}

func TestSetVerificationModeCreatesStub(t *testing.T) {
	path := filepath.Join(t.TempDir(), "missing", "config.yaml")
	if err := SetVerificationMode(path, "advisory"); err != nil {
		t.Fatal(err)
	}
	snap := Load(filepath.Dir(filepath.Dir(path)))
	// Load looks at root/.devcouncil/config.yaml; this test wrote a custom path.
	body, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(body), "mode: advisory") {
		t.Fatalf("%s", body)
	}
	_ = snap
}

func writeCfg(t *testing.T, root, body string) string {
	t.Helper()
	dir := filepath.Join(root, ".devcouncil")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(dir, "config.yaml")
	if err := os.WriteFile(path, []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
	return path
}

func TestMissingConfigIsTheOnlyDefault(t *testing.T) {
	snap := Resolve(t.TempDir())
	if snap.VerificationMode != ModeOff || snap.VerificationOrigin != OriginDefault || snap.Error != "" {
		t.Fatalf("missing config = %+v, want off/default with no error", snap)
	}
}

func TestUnreadableConfigFailsClosed(t *testing.T) {
	if os.Geteuid() == 0 {
		t.Skip("root reads chmod-000 files")
	}
	root := t.TempDir()
	path := writeCfg(t, root, "gates:\n  mode: off\n")
	if err := os.Chmod(path, 0); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.Chmod(path, 0o644) })
	for _, snap := range []Snapshot{Load(root), Resolve(root)} {
		if snap.VerificationMode != ModeEnforce || snap.VerificationOrigin != OriginInvalid {
			t.Fatalf("unreadable config = %+v, want enforce/invalid", snap)
		}
		if snap.Error == "" || !snap.ConfigPresent {
			t.Fatalf("unreadable config must report its error and presence: %+v", snap)
		}
	}
}

func TestScalarGatesAndUnknownModeFailClosed(t *testing.T) {
	for name, body := range map[string]string{
		"scalar gates": "gates: off\n",
		"typo mode":    "gates:\n  mode: enforced\n",
	} {
		root := t.TempDir()
		writeCfg(t, root, body)
		snap := Resolve(root)
		if snap.VerificationMode != ModeEnforce || snap.VerificationOrigin != OriginInvalid || snap.Error == "" {
			t.Errorf("%s = %+v, want enforce/invalid with an error", name, snap)
		}
	}
}

// Resolve is read per call, so `gate set` reaches a host that is already
// running, and chmod-ing a file that was read fine a moment ago is noticed.
func TestResolveFollowsTheFileWithoutRestart(t *testing.T) {
	root := t.TempDir()
	path := writeCfg(t, root, "gates:\n  mode: off\n")
	if got := Resolve(root).VerificationMode; got != ModeOff {
		t.Fatalf("first resolve = %s, want off", got)
	}
	if err := SetVerificationMode(path, "enforce"); err != nil {
		t.Fatal(err)
	}
	if got := Resolve(root).VerificationMode; got != ModeEnforce {
		t.Fatalf("after gate set = %s, want enforce", got)
	}
	if os.Geteuid() != 0 {
		if err := os.Chmod(path, 0); err != nil {
			t.Fatal(err)
		}
		t.Cleanup(func() { _ = os.Chmod(path, 0o644) })
		if got := Resolve(root); got.VerificationOrigin != OriginInvalid {
			t.Fatalf("after chmod 000 a cached read was served: %+v", got)
		}
	}
}
