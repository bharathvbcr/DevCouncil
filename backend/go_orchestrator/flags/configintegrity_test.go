package flags

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func loadDoc(t *testing.T, doc string) error {
	t.Helper()
	r := New()
	if err := DefineHarnessFlags(r); err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(t.TempDir(), "config.yaml")
	if err := os.WriteFile(path, []byte(doc), 0o644); err != nil {
		t.Fatal(err)
	}
	_, err := LoadConfigFile(r, path)
	return err
}

// A flow mapping standing where nested keys belong was stored as one raw
// string under the bare namespace, skipped as foreign, and the setting inside
// it (here: agent grants switched off) was silently lost.
func TestAFlowMappingUnderAHarnessNamespaceIsRefusedWithItsLine(t *testing.T) {
	for _, doc := range []string{
		"harness: {posture: yolo}\n",
		"grants: {agent: {enabled: false}}\n",
		"policy: {file: {mode: enforce}}\n",
		"harness: [a, b]\n",
	} {
		err := loadDoc(t, doc)
		if err == nil {
			t.Fatalf("%q loaded without an error and dropped its settings", doc)
		}
		if !strings.Contains(err.Error(), "(line 1)") {
			t.Errorf("%q: error %q does not name line 1", doc, err)
		}
	}
	for _, ok := range []string{"harness: {}\n", "grants: []\n", "languages: [go, rust]\nother: {a: b}\n"} {
		if err := loadDoc(t, ok); err != nil {
			t.Errorf("%q must stay legal: %v", ok, err)
		}
	}
}

func TestABlockListUnderAScalarFlagIsRefused(t *testing.T) {
	err := loadDoc(t, "llm:\n  local:\n    model:\n      - a\n      - b\n")
	if err == nil || !strings.Contains(err.Error(), "line 4") {
		t.Fatalf("a list under llm.local.model must be refused with its line, got %v", err)
	}
	if err := loadDoc(t, "languages:\n  - go\n  - rust\n"); err != nil {
		t.Fatalf("a list under a foreign key must stay legal: %v", err)
	}
}

// An unclosed quote must not let a later quote swallow the settings between.
func TestAnUnterminatedQuoteIsRefusedBeforeALaterQuoteCanCloseIt(t *testing.T) {
	doc := "llm.local.model: \"gpt\ngates.mode: enforce\nllm.local.base_url: \"http://x\"\n"
	if got := parseErr(t, doc); !strings.Contains(got, "line 2") || !strings.Contains(got, "line 1") {
		t.Fatalf("error %q must name the opening line and the line that proves the quote is open", got)
	}
	if got := parseErr(t, "a.b: \"never closed\n"); !strings.Contains(got, "line 1") {
		t.Fatalf("EOF case must name line 1, got %q", got)
	}
}

func TestTrailingJunkAfterAMultilineClosingQuoteIsRefused(t *testing.T) {
	if got := parseErr(t, "a.b: \"first\n  second\" junk\n"); !strings.Contains(got, "line 2") {
		t.Fatalf("error %q must name line 2", got)
	}
	if _, err := ParseConfig(strings.NewReader("a.b: \"first\n  second\" # note\n")); err != nil {
		t.Fatalf("a comment after the closing quote is legal: %v", err)
	}
}

func TestCROnlyLineEndingsAreRefused(t *testing.T) {
	if got := parseErr(t, "a.b: 1\rgates.mode: enforce\r"); !strings.Contains(got, "line 1") {
		t.Fatalf("error %q must name line 1", got)
	}
	if _, err := ParseConfig(strings.NewReader("a.b: 1\r\ngates.mode: enforce\r\n")); err != nil {
		t.Fatalf("CRLF stays legal: %v", err)
	}
}

// FuzzParseConfig is the audit's harness, committed. Invariants: no panic, and
// anything accepted keeps every key's value free of the file's own structure
// (no bare CR, which only a mis-split line can carry).
func FuzzParseConfig(f *testing.F) {
	for _, seed := range []string{
		"gates:\n  mode: enforce\n",
		"llm.local.model: \"gpt\ngates.mode: enforce\n",
		"a: 1\rb: 2\r",
		"harness: {posture: yolo}\n",
		"k:\n  - a\n",
		"a: \"x\n  y\" z\n",
		"\ufeffa: 1\n",
	} {
		f.Add(seed)
	}
	f.Fuzz(func(t *testing.T, doc string) {
		layer, err := ParseConfigLayer(strings.NewReader(doc))
		if err != nil {
			return
		}
		for k, v := range layer.Values {
			if strings.ContainsRune(v, '\r') || strings.ContainsRune(k, '\r') {
				t.Fatalf("accepted a bare CR in %q=%q from %q", k, v, doc)
			}
			if _, ok := layer.Lines[k]; !ok {
				t.Fatalf("key %q has no line", k)
			}
		}
	})
}
