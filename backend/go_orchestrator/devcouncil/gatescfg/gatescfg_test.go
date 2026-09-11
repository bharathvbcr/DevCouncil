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

func TestNormalizeUnknownIsOffInvalid(t *testing.T) {
	mode, origin := Normalize("yolo")
	if mode != ModeOff || origin != OriginInvalid {
		t.Fatalf("Normalize(yolo) = %s/%s, want off/invalid", mode, origin)
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
	if snap.VerificationMode != ModeOff {
		t.Fatalf("broken YAML must not enforce: %+v", snap)
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
