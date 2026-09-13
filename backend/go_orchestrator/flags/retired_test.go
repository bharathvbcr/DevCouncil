package flags

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// The four llm.provider.*.enabled keys were declared, documented as enabling an
// adapter, and read by nothing. This pins their removal: a key that is defined
// is a key `manvi flags` lists and an operator will set.
func TestTheProviderEnableSwitchesAreGone(t *testing.T) {
	reg := New()
	if err := DefineHarnessFlags(reg); err != nil {
		t.Fatal(err)
	}
	for _, key := range []string{
		"llm.provider.anthropic.enabled",
		"llm.provider.gemini.enabled",
		"llm.provider.xai.enabled",
		"llm.provider.local.enabled",
	} {
		if _, ok := reg.Def(key); ok {
			t.Errorf("%s is still defined, and nothing in the harness reads it: "+
				"llm.provider.default plus the provider's credential decide which adapter runs", key)
		}
	}
}

// Removing the key is only half of it. LoadEnv iterates the flags that are
// defined, so a variable naming a removed one is not refused — it is not looked
// at, which is the same silence the operator already had and the reason they
// never learned the setting did nothing.
func TestRetiredEnvNamesEverySettingThatIsGone(t *testing.T) {
	got := RetiredEnv([]string{
		"MANVI_LLM_PROVIDER_LOCAL_ENABLED=false",
		"MANVI_LLM_PROVIDER_GEMINI_ENABLED=true",
		"MANVI_LLM_LOCAL_MODEL=qwen3.8:27b-mlx",
		"HOME=/home/x",
	})
	if len(got) != 2 {
		t.Fatalf("reported %d retired variables, want 2: %+v", len(got), got)
	}
	for _, r := range got {
		if r.Why == "" {
			t.Errorf("%s is reported with no explanation; an operator is told to unset it and not why", r.Env)
		}
	}
	if got[0].Env != "MANVI_LLM_PROVIDER_GEMINI_ENABLED" || got[1].Env != "MANVI_LLM_PROVIDER_LOCAL_ENABLED" {
		t.Errorf("unexpected variables reported: %+v", got)
	}
	if len(RetiredEnv([]string{"MANVI_LLM_LOCAL_MODEL=x", "HOME=/home/x"})) != 0 {
		t.Error("a live setting was reported as retired")
	}
}

// A config file is refused either way, but "unknown key" reads as a typo the
// operator should correct rather than as a setting somebody removed.
func TestARetiredKeyInTheConfigFileSaysItWasRemoved(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "config.yaml")
	if err := os.WriteFile(path, []byte(
		"llm.provider.local.enabled: false\nllm.local.model: qwen3.8:27b-mlx\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	_, err := NewHarnessRegistry(path)
	if err == nil {
		t.Fatal("a config file setting a removed key was accepted")
	}
	for _, want := range []string{"llm.provider.local.enabled", "no longer has", "llm.provider.default"} {
		if !strings.Contains(err.Error(), want) {
			t.Errorf("the refusal does not mention %q: %v", want, err)
		}
	}
}

// The two verification switches were declared, described as promoting a gap to
// blocking and as enabling stub/effort detection, and read by nothing: the
// gates they name live in dcverify, and nothing consults either key to decide
// whether they run or what they block. A team that set `enforce: true` got no
// change in behaviour, which is the worst of the three possible outcomes — a
// check that could not run reported the same clean result as one that ran and
// passed.
//
// This held when verify.Run() spawned no verifier at all, and it still holds
// now that TASK-P7-1 has it spawn dcverify: the gates are reached by finding
// the binary, and what blocks is fixed in verify/rigor.go. That the wiring
// landed without either key coming back is the evidence they were dead.
func TestTheVerificationSwitchesAreGone(t *testing.T) {
	reg := New()
	if err := DefineHarnessFlags(reg); err != nil {
		t.Fatal(err)
	}
	for _, key := range []string{
		"verify.diff_coverage.enforce",
		"verify.rigor.enabled",
	} {
		if _, ok := reg.Def(key); ok {
			t.Errorf("%s is still defined, and nothing in this host reads it: "+
				"the gate it names runs in dcverify, which verify.Run reaches by finding the binary, "+
				"not by consulting this key", key)
		}
	}
}

// Removing the Def is half of it, for the reason the provider switches above
// record: LoadEnv iterates the defined flags, so a variable naming a removed
// one is passed over in the same silence the operator already had.
func TestTheVerificationSwitchesAreNamedAsRetired(t *testing.T) {
	got := RetiredEnv([]string{
		"MANVI_VERIFY_RIGOR_ENABLED=true",
		"MANVI_VERIFY_DIFF_COVERAGE_ENFORCE=true",
		"HOME=/home/x",
	})
	if len(got) != 2 {
		t.Fatalf("reported %d retired variables, want 2: %+v", len(got), got)
	}
	for _, r := range got {
		if !strings.Contains(r.Why, "dcverify") {
			t.Errorf("%s is retired without naming where the gate actually runs: %q", r.Env, r.Why)
		}
	}

	dir := t.TempDir()
	path := filepath.Join(dir, "config.yaml")
	if err := os.WriteFile(path, []byte("verify.diff_coverage.enforce: true\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	_, err := NewHarnessRegistry(path)
	if err == nil {
		t.Fatal("a config file setting the removed enforce switch was accepted")
	}
	if !strings.Contains(err.Error(), "verify.diff_coverage.enforce") ||
		!strings.Contains(err.Error(), "no longer has") {
		t.Errorf("the refusal does not name the removed setting: %v", err)
	}
}

// The DevCouncil config spellings are a different matter from the harness keys
// above, and must stay *silent* rather than become refusals.
//
// `verification.*` is DevCouncil core config, shared with Manvi through
// .devcouncil/config.yaml and consumed outside this tree. Most of that
// namespace was never aliased onto a harness flag and has always been passed
// over; refusing the two that were would break the real shared file this
// repository ships.
func TestTheDevCouncilVerificationSpellingsArePassedOverNotRefused(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "config.yaml")
	if err := os.WriteFile(path, []byte(
		"verification.rigor.enabled: true\n"+
			"verification.diff_coverage.enforce: true\n"+
			"verification.rigor.stub_detection: hard\n"+
			"llm.local.model: qwen3.8:27b-mlx\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	r, err := NewHarnessRegistry(path)
	if err != nil {
		t.Fatalf("the shared DevCouncil spellings were refused rather than passed over: %v", err)
	}
	// Passed over means exactly that: not aliased onto a harness flag either.
	for _, key := range []string{"verify.rigor.enabled", "verify.diff_coverage.enforce"} {
		if _, ok := r.Def(key); ok {
			t.Errorf("%s came back as a defined flag; the alias was supposed to go with it", key)
		}
	}
	if model, _, err := r.String(LLMLocalModel); err != nil || model != "qwen3.8:27b-mlx" {
		t.Errorf("a live setting in the same file stopped loading: %q (%v)", model, err)
	}
}

// The synthetic file above is a copy of the real one, and a copy is the thing
// that drifts. This repository ships .devcouncil/config.yaml with both
// verification.* keys set, so the removal is checked against the actual file a
// checkout loads at startup rather than against this test's idea of it.
//
// It skips outside a checkout instead of passing, because a refusal that could
// not be attempted is not a refusal that did not happen.
func TestTheRealSharedConfigStillLoads(t *testing.T) {
	wd, err := os.Getwd()
	if err != nil {
		t.Fatal(err)
	}
	var path string
	for dir := wd; ; {
		candidate := filepath.Join(dir, ".devcouncil", "config.yaml")
		if _, err := os.Stat(candidate); err == nil {
			path = candidate
			break
		}
		parent := filepath.Dir(dir)
		if parent == dir {
			t.Skip("not running from a DevCouncil checkout")
		}
		dir = parent
	}

	raw, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	// Guard the premise: if the shipped file stops setting these, this test
	// would pass while checking nothing.
	for _, want := range []string{"diff_coverage:", "enforce:", "rigor:"} {
		if !strings.Contains(string(raw), want) {
			t.Fatalf("%s no longer contains %q; this test is no longer exercising the removal", path, want)
		}
	}

	if _, err := NewHarnessRegistry(path); err != nil {
		t.Fatalf("the shipped config was refused after the verification keys were removed: %v", err)
	}
}
