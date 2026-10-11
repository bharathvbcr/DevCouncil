package policy

import (
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/fnmatch"
)

// APFS folds U+017F (long s) to "s" and U+212A (Kelvin sign) to "k", so these
// spellings open credential files the patterns were written for. Write, read
// and the read classifier all refuse them, whether or not the file exists yet.
func TestFoldedSpellingsOfSecretsAreRefusedForWriteAndRead(t *testing.T) {
	root := t.TempDir()
	task := &dc.Task{ID: "T", PlannedFiles: []dc.PlannedFile{{Path: "src/a.go", AllowedChange: dc.ChangeModify}}}
	gate := FileGate{Root: root, HardRules: true}
	for _, name := range []string{
		"id_rſa", "ſecrets/x", "ſecrets/new.txt", "a/ſecrets/new.txt", "ID_RſA", "x.\u212aey",
		"credentialſ/x", ".npmrc", "x.pem",
	} {
		w := gate.EvaluateFileChange(name, task, dc.OpWrite, false)
		if w.Action != Deny || w.Rule != RuleSecretPath || w.Severity != Hard {
			t.Errorf("write %q = %s/%s", name, w.Action, w.Rule)
		}
		r := gate.EvaluateRead(name, task)
		if r.Action != Deny || r.Rule != RuleSecretRead {
			t.Errorf("read %q = %s/%s", name, r.Action, r.Rule)
		}
		if got := ClassifyRead(root, name); got != ReadSecretPath {
			t.Errorf("ClassifyRead(%q) = %v", name, got)
		}
		if !SecretPathSpelling(name) {
			t.Errorf("SecretPathSpelling(%q) = false", name)
		}
	}
	for _, name := range []string{".git/config", ".GIT/config", ".Claude/agents/x.md"} {
		if d := gate.EvaluateFileChange(name, task, dc.OpWrite, false); d.Rule != RuleRestrictedPath {
			t.Errorf("write %q = %s/%s, want %s", name, d.Action, d.Rule, RuleRestrictedPath)
		}
	}
	for _, name := range []string{"src/a.go", "docs/readme.md", "ſrc/a.go"} {
		if SecretPathSpelling(name) {
			t.Errorf("SecretPathSpelling(%q) = true for a name no pattern covers", name)
		}
	}
}

// The restricted rung's prefix test folds with the same table the glob does.
func TestRestrictedPrefixFoldsLongS(t *testing.T) {
	mt := matcherOf(nil)
	if !matchesRestricted(mt, ".GIT/hooks/x") {
		t.Error(".GIT/hooks/x must be restricted")
	}
	// Folding must not widen: a different letter is a different directory.
	if matchesRestricted(mt, ".gitx/hooks") {
		t.Error(".gitx is not .git")
	}
}

// A path that exists is matched by the name the filesystem stores it under.
// Where the volume is case-insensitive a different case is the same file, and
// the stored spelling is what a pattern must be checked against.
func TestOnDiskSpellingReportsTheStoredName(t *testing.T) {
	root := t.TempDir()
	if err := os.MkdirAll(filepath.Join(root, "Config", "Keys"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "Config", "Keys", "Deploy.TXT"), []byte("x"), 0o600); err != nil {
		t.Fatal(err)
	}
	if got, differs := onDiskSpelling(root, "Config/Keys/Deploy.TXT"); differs {
		t.Errorf("exact spelling reported as different: %q", got)
	}
	if got, differs := onDiskSpelling(root, "Config/Keys/new.txt"); differs || got != "Config/Keys/new.txt" {
		t.Errorf("a path that does not exist has no stored spelling: %q %v", got, differs)
	}
	if _, err := os.Lstat(filepath.Join(root, "config")); err != nil {
		t.Skip("case-sensitive volume: no second spelling of an existing path exists to test")
	}
	got, differs := onDiskSpelling(root, "config/KEYS/deploy.txt")
	if !differs || got != "Config/Keys/Deploy.TXT" {
		t.Errorf("onDiskSpelling = %q (differs %v), want Config/Keys/Deploy.TXT", got, differs)
	}
}

// With the stored name checked, the pattern a test cannot fold to still sees an
// existing file: a symlink-free alias reached by a spelling the pattern list
// does not cover.
func TestStoredNameIsCheckedAgainstSecretPatterns(t *testing.T) {
	root := t.TempDir()
	if err := os.WriteFile(filepath.Join(root, "id_rsa"), []byte("k"), 0o600); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Lstat(filepath.Join(root, "ID_RSA")); err != nil {
		t.Skip("case-sensitive volume")
	}
	gate := FileGate{Root: root, HardRules: true}
	task := &dc.Task{ID: "T", PlannedFiles: []dc.PlannedFile{{Path: "ID_RSA", AllowedChange: dc.ChangeModify}}}
	if d := gate.EvaluateFileChange("ID_RSA", task, dc.OpWrite, false); d.Rule != RuleSecretPath {
		t.Errorf("write = %s/%s", d.Action, d.Rule)
	}
}

// testdata/secret-paths-fold.tsv is read by rust/dc-glob's is_secret_path as
// well: the two engines answer the same rows or one of them leaks a credential.
func TestSecretPathFoldFixtureHeldByGo(t *testing.T) {
	_, file, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("cannot locate test source")
	}
	raw, err := os.ReadFile(filepath.Join(filepath.Dir(file), "..", "..", "..", "testdata", "secret-paths-fold.tsv"))
	if err != nil {
		t.Fatal(err)
	}
	checked := 0
	for i, line := range strings.Split(string(raw), "\n") {
		if strings.HasPrefix(line, "#") || strings.TrimSpace(line) == "" {
			continue
		}
		cols := strings.Split(line, "\t")
		if len(cols) != 2 {
			t.Fatalf("line %d is malformed: %q", i+1, line)
		}
		if got := fnmatch.MatchAnyFold(SecretPathPatterns, cols[0]); got != (cols[1] == "true") {
			t.Errorf("MatchAnyFold(SecretPathPatterns, %q) = %v, fixture says %s (line %d)", cols[0], got, cols[1], i+1)
		}
		checked++
	}
	if checked < 20 {
		t.Fatalf("only %d rows loaded", checked)
	}
}
