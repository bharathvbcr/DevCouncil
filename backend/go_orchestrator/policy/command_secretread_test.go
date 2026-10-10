package policy

import (
	"os"
	"path/filepath"
	"testing"
)

func secretReadRoot(t *testing.T) string {
	t.Helper()
	root := t.TempDir()
	for name, body := range map[string]string{
		".env":              "API_KEY=x",
		"secrets/token.txt": "t",
		"README.md":         "readme",
		"notes.txt":         "notes",
		"src/a.go":          "package a",
	} {
		path := filepath.Join(root, name)
		if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, []byte(body), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	if err := os.Symlink(".env", filepath.Join(root, "link-to-env")); err != nil {
		t.Fatal(err)
	}
	return root
}

// `cat .env` was a soft command.no_lease that the dev posture demoted, so the
// shell read what read_file refuses. A command whose operands name a credential
// path is a hard refusal in every posture but yolo.
func TestShellCommandsNamingASecretPathAreRefusedHard(t *testing.T) {
	root := secretReadRoot(t)
	gate := CommandGate{HardRules: true, Root: root}

	for _, tool := range []string{"cat", "head", "tail", "less", "base64"} {
		for _, target := range []string{".env", "secrets/token.txt", "./.env", "sub/../.env", `".env"`, `'.e'nv`, "link-to-env"} {
			command := tool + " " + target
			d := gate.EvaluateCommand(command, nil)
			if d.Action != Deny || d.Severity != Hard || d.Rule != RuleCommandSecretRead {
				t.Errorf("%q: %s %s/%s, want a hard %s", command, d.Action, d.Rule, d.Severity, RuleCommandSecretRead)
			}
		}
	}
	refused := []string{
		"cp .env notes.txt",
		"cp secrets/token.txt out.txt",
		"head -n 5 .env",
		"tail -f -n1 secrets/token.txt",
		"cat < .env",
		"cat <.env",
		"base64 < secrets/token.txt",
		"tr a b < .env",
		"dd if=.env of=notes.txt",
		"cat --file=.env",
		"cat .e*",
		"cat secrets/*",
		"cat README.md .env",
		"git cat-file -p HEAD:.env",
		"git grep API_KEY -- .env",
		"cat $F",
		"cat $(echo .env)",
		"git add . && cat .env",
		"sed -n p .env",
		"awk 1 .env",
		"grep API_KEY .env",
	}
	for _, command := range refused {
		d := gate.EvaluateCommand(command, nil)
		if d.Action != Deny || d.Severity != Hard {
			t.Errorf("%q was %s %s/%s, want a hard refusal", command, d.Action, d.Rule, d.Severity)
		}
	}
	// And the rule is the one that names them, where it is the one that decides.
	if d := gate.EvaluateCommand("cat .env", nil); d.Rule != RuleCommandSecretRead {
		t.Errorf("rule = %s", d.Rule)
	}
	if d := gate.EvaluateCommand("cat .env", nil); d.Target == "" || d.Reason == "" {
		t.Errorf("the refusal names nothing: %+v", d)
	}
}

// The rule is about reading a credential, so commands that only mention one, and
// reads of ordinary files, are untouched.
func TestShellCommandsNotReadingASecretAreNotRefusedByTheSecretRule(t *testing.T) {
	root := secretReadRoot(t)
	gate := CommandGate{HardRules: true, Root: root, GlobalAllowedCommands: []string{"*"}}

	for _, command := range []string{
		"cat README.md",
		"head -n 3 src/a.go",
		"cp README.md out.md",
		`echo ".env" >> .gitignore`,
		"printf '%s' .env",
		"ls .env secrets",
		"rm .env",
		"touch secrets/new.txt",
		"git add src/a.go",
		"git status",
		"cat docs/env-setup.md",
		"cat environment.go",
		"cat > out.txt",
		"echo hi > .env",
	} {
		d := gate.EvaluateCommand(command, nil)
		if d.Rule == RuleCommandSecretRead {
			t.Errorf("%q was refused as a secret read: %s", command, d.Reason)
		}
	}
}

// Yolo is the one posture that turns the hard rules off.
func TestSecretReadRuleIsOffOnlyWhenHardRulesAre(t *testing.T) {
	root := secretReadRoot(t)
	gate := CommandGate{HardRules: false, Root: root, GlobalAllowedCommands: []string{"cat *"}}
	d := gate.EvaluateCommand("cat .env", nil)
	if d.Rule == RuleCommandSecretRead {
		t.Fatalf("a gate with hard rules off applied the hard secret rule: %+v", d)
	}
}

// With no root there is no filesystem to settle a path against, and the rule
// falls back to the spelling.
func TestSecretReadRuleWithoutARoot(t *testing.T) {
	gate := CommandGate{HardRules: true}
	if d := gate.EvaluateCommand("cat .env", nil); d.Rule != RuleCommandSecretRead {
		t.Fatalf("cat .env with no root = %s %s", d.Action, d.Rule)
	}
}

func TestWithholdDiffUsesTheCallersRule(t *testing.T) {
	diff := "diff --git a/notes.txt b/notes.txt\nnew file mode 100644\n--- /dev/null\n+++ b/notes.txt\n@@ -0,0 +1 @@\n+hello\n" +
		"diff --git a/ok.txt b/ok.txt\nnew file mode 100644\n--- /dev/null\n+++ b/ok.txt\n@@ -0,0 +1 @@\n+fine\n"
	kept, withheld := WithholdDiff(diff, func(name string) string {
		if name == "notes.txt" {
			return "custom.rule"
		}
		return ""
	})
	if len(withheld) == 0 || withheld[0].Path != "b/notes.txt" || withheld[0].Rule != "custom.rule" {
		t.Fatalf("withheld = %+v", withheld)
	}
	if want := "diff --git a/ok.txt"; len(kept) == 0 || kept[:len(want)] != want {
		t.Fatalf("kept = %q", kept)
	}
	// The secret-only form keeps its old behaviour.
	if _, names := WithholdSecretDiff("diff --git a/.env b/.env\n--- /dev/null\n+++ b/.env\n@@ -0,0 +1 @@\n+K=v\n"); len(names) == 0 {
		t.Fatal("WithholdSecretDiff stopped withholding .env")
	}
}
