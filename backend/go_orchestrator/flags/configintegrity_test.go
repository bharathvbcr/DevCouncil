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

// writeRepoConfig writes doc as a repository's .devcouncil/config.yaml.
func writeRepoConfig(t *testing.T, doc string) string {
	t.Helper()
	path := filepath.Join(t.TempDir(), "config.yaml")
	if err := os.WriteFile(path, []byte(doc), 0o644); err != nil {
		t.Fatal(err)
	}
	return path
}

// TestRepositoryConfigCannotWeakenASafetyFlag is decision D-1. The repository's
// config.yaml is a file a clone ships and an agent can write, so a value in it
// that loosens a safety flag is the repository choosing its own gate. Each of
// these used to load with origin "config" and switch the gate down; each must
// now be refused, naming the file, the line and the key.
func TestRepositoryConfigCannotWeakenASafetyFlag(t *testing.T) {
	for _, tc := range []struct{ name, line, key string }{
		{"hard rules off", "policy.hard_rules.enabled: false", PolicyHardRules},
		{"hard rules off, word spelling", "policy.hard_rules.enabled: off", PolicyHardRules},
		{"yolo posture", "harness.posture: yolo", HarnessPosture},
		{"file gate off", "policy.file.mode: off", PolicyFileMode},
		{"file gate advisory", "policy.file.mode: advisory", PolicyFileMode},
		{"command gate off", "policy.command.mode: off", PolicyCommandMode},
		{"agent command grants on", "grants.agent.allow_commands: true", GrantsAgentCommands},
		{"grant reasons optional", "grants.require_reason: false", GrantsRequireReason},
		{"longer agent grants", "grants.agent.max_ttl: 24h", GrantsAgentMaxTTL},
		{"session-log assert off", "log.model_visible_assert: false", LogModelVisibleAssert},
	} {
		t.Run(tc.name, func(t *testing.T) {
			path := writeRepoConfig(t, "# repository settings\nllm.local.model: qwen\n"+tc.line+"\n")
			reg, err := NewHarnessRegistry(path)
			if err == nil {
				v, _ := reg.Lookup(tc.key)
				t.Fatalf("%q was accepted from the repository config: %s = %q (origin %s)", tc.line, tc.key, v.Raw, v.Origin)
			}
			for _, want := range []string{path, tc.key + " (line 3)", "only tighten"} {
				if !strings.Contains(err.Error(), want) {
					t.Errorf("refusal %q does not name %q", err, want)
				}
			}
		})
	}

	// The nested spelling reaches the same key and is refused the same way.
	path := writeRepoConfig(t, "harness:\n  posture: yolo\n")
	if _, err := NewHarnessRegistry(path); err == nil || !strings.Contains(err.Error(), HarnessPosture+" (line 2)") {
		t.Fatalf("nested harness.posture: yolo must be refused at line 2, got %v", err)
	}
}

// TestEveryWeakeningLineIsNamed: a refusal that names only the first offender
// sends the operator round the loop once per line.
func TestEveryWeakeningLineIsNamed(t *testing.T) {
	path := writeRepoConfig(t, "harness.posture: yolo\npolicy.hard_rules.enabled: false\n")
	_, err := NewHarnessRegistry(path)
	if err == nil {
		t.Fatal("a repository config loosening two safety flags was accepted")
	}
	for _, want := range []string{HarnessPosture + " (line 1)", PolicyHardRules + " (line 2)", EnvKey(HarnessPosture), EnvKey(PolicyHardRules)} {
		if !strings.Contains(err.Error(), want) {
			t.Errorf("refusal %q does not name %q", err, want)
		}
	}
}

// TestRepositoryConfigMayTightenASafetyFlag is the other half of D-1: the rule
// is "only tighten", not "never set". A repository that wants a stricter gate
// than the operator's default gets it.
func TestRepositoryConfigMayTightenASafetyFlag(t *testing.T) {
	path := writeRepoConfig(t, `harness:
  posture: strict
policy.file.mode: enforce
policy.hard_rules.enabled: true
grants.agent.enabled: false
grants.agent.max_ttl: 5m
gates:
  mode: enforce
`)
	reg, err := NewHarnessRegistry(path)
	if err != nil {
		t.Fatalf("a tightening repository config was refused: %v", err)
	}
	if v, _ := reg.Lookup(HarnessPosture); v.Raw != PostureStrict || v.Origin != OriginConfig {
		t.Errorf("harness.posture = %+v, want strict from config", v)
	}
	if mode, _, _ := EffectiveGateMode(reg, PolicyCommandMode); mode != ModeEnforce {
		t.Errorf("command gate = %q under a strict posture, want enforce", mode)
	}
	if on, _, _ := EffectiveHardRules(reg); !on {
		t.Error("hard rules must run")
	}
	if b, origin, _ := reg.Bool(GrantsAgentEnabled); b || origin != OriginConfig {
		t.Errorf("grants.agent.enabled = %v (%s), want false from config", b, origin)
	}
	if d, _, _ := reg.Duration(GrantsAgentMaxTTL); d.String() != "5m0s" {
		t.Errorf("grants.agent.max_ttl = %s, want the shorter 5m", d)
	}
	// Restating a safety flag at the value already in force tightens nothing
	// and loosens nothing; it stays legal.
	if _, err := NewHarnessRegistry(writeRepoConfig(t, "harness.posture: dev\n")); err != nil {
		t.Errorf("restating the default posture was refused: %v", err)
	}
}

// TestTheEnvironmentKeepsOperatorAuthority: the rule narrows what the repository
// may say, not what the operator may. A weakening value from the environment
// still applies over a stricter repository file, and Weakened still names it
// with the layer that set it.
func TestTheEnvironmentKeepsOperatorAuthority(t *testing.T) {
	t.Setenv(EnvKey(PolicyHardRules), "false")
	reg, err := NewHarnessRegistry(writeRepoConfig(t, "harness.posture: strict\n"))
	if err != nil {
		t.Fatal(err)
	}
	on, origin, err := EffectiveHardRules(reg)
	if err != nil || on || origin != OriginEnv {
		t.Fatalf("hard rules = %v (%s, err %v), want off from env", on, origin, err)
	}
	var found bool
	for _, w := range reg.Weakened() {
		if w.Key == PolicyHardRules && w.Origin == OriginEnv {
			found = true
		}
	}
	if !found {
		t.Fatalf("Weakened = %+v, want policy.hard_rules.enabled reported from env", reg.Weakened())
	}
}

// TestScopeDecidesWhoMayLoosen covers the layer API a host builds its own
// files with (Manvi reads its user config this way). An operator-scope layer
// may loosen; a layer that names no scope gets the repository's rule, because
// the safe reading of "nobody said where this came from" is the strict one;
// and a repository layer cannot undo a stricter value an operator layer
// already put in place.
func TestScopeDecidesWhoMayLoosen(t *testing.T) {
	op := testRegistry(t)
	if err := op.LoadLayer(ConfigLayer{Values: map[string]string{HarnessPosture: PostureYolo}, Scope: ScopeOperator}); err != nil {
		t.Fatalf("operator scope must be able to choose yolo: %v", err)
	}
	if v, _ := op.Lookup(HarnessPosture); v.Raw != PostureYolo || v.Origin != OriginConfig {
		t.Fatalf("posture = %+v, want yolo from config", v)
	}

	unlabelled := testRegistry(t)
	err := unlabelled.LoadLayer(ConfigLayer{Values: map[string]string{PolicyHardRules: "false"}, Lines: map[string]int{PolicyHardRules: 7}})
	if err == nil || !strings.Contains(err.Error(), PolicyHardRules+" (line 7)") {
		t.Fatalf("an unlabelled layer turning hard rules off must be refused at its line, got %v", err)
	}
	if v, _ := unlabelled.Lookup(PolicyHardRules); v.Origin != OriginDefault {
		t.Fatalf("a refused layer applied a value: %+v", v)
	}

	ordered := testRegistry(t)
	if err := ordered.LoadLayer(ConfigLayer{Values: map[string]string{HarnessPosture: PostureStrict}, Scope: ScopeOperator}); err != nil {
		t.Fatal(err)
	}
	if err := ordered.LoadLayer(ConfigLayer{Values: map[string]string{HarnessPosture: PostureDev}, Scope: ScopeRepository}); err == nil {
		t.Fatal("a repository layer moved the posture from the operator's strict back to dev")
	}
	if v, _ := ordered.Lookup(HarnessPosture); v.Raw != PostureStrict {
		t.Fatalf("posture = %q after a refused repository layer, want the operator's strict", v.Raw)
	}
}

// TestLadderDeclarationsAreChecked: a strictness order that is wrong, partial,
// or attached to a flag nothing checks is a rule that does not hold, so Define
// refuses each — and a safety enum must have one, because its values have no
// order of their own.
func TestLadderDeclarationsAreChecked(t *testing.T) {
	modes := []string{ModeEnforce, ModeAdvisory, ModeOff}
	for _, tc := range []struct {
		name string
		def  Def
	}{
		{"safety enum without a ladder", Def{Key: "a.mode", Kind: KindEnum, Values: modes, Default: ModeEnforce, Mutable: HumanOnly, Safety: true}},
		{"ladder missing a value", Def{Key: "a.mode", Kind: KindEnum, Values: modes, Ladder: modes[:2], Default: ModeEnforce, Mutable: HumanOnly, Safety: true}},
		{"ladder with a stranger", Def{Key: "a.mode", Kind: KindEnum, Values: modes, Ladder: []string{ModeEnforce, ModeAdvisory, "maybe"}, Default: ModeEnforce, Mutable: HumanOnly, Safety: true}},
		{"ladder with a repeat", Def{Key: "a.mode", Kind: KindEnum, Values: modes, Ladder: []string{ModeEnforce, ModeEnforce, ModeOff}, Default: ModeEnforce, Mutable: HumanOnly, Safety: true}},
		{"ladder on a non-safety flag", Def{Key: "a.mode", Kind: KindEnum, Values: modes, Ladder: modes, Default: ModeEnforce, Mutable: HumanOnly}},
		{"bool ladder spelt loosely", Def{Key: "a.on", Kind: KindBool, Ladder: []string{"yes", "no"}, Default: "true", Mutable: HumanOnly, Safety: true}},
		{"ladder on a string", Def{Key: "a.s", Kind: KindString, Ladder: []string{"x"}, Default: "x", Mutable: HumanOnly, Safety: true}},
	} {
		if err := New().Define(tc.def); err == nil {
			t.Errorf("%s: Define accepted it", tc.name)
		}
	}
}
