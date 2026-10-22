package policy

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// A program the gate does not model is asked about every operand. The root is
// a real temporary tree so symlinks resolve as they do in a checkout.
func TestUnmodelledProgramOperandsAreMatchedAgainstTheHardRules(t *testing.T) {
	root := t.TempDir()
	outside := t.TempDir()
	for _, dir := range []string{"src", ".git/hooks", ".devcouncil", ".claude"} {
		if err := os.MkdirAll(filepath.Join(root, dir), 0o755); err != nil {
			t.Fatal(err)
		}
	}
	if err := os.WriteFile(filepath.Join(outside, "target"), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(outside, filepath.Join(root, "src", "link")); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(filepath.Join(root, ".git"), filepath.Join(root, "src", "gitlink")); err != nil {
		t.Fatal(err)
	}
	gate := CommandGate{HardRules: true, Root: root}

	refused := []string{
		"sqlite3 .devcouncil/state.sqlite 'drop table x'",
		"sqlite3 ../outside.db .tables",
		"prettier --write ../outside.js",
		"formatter /etc/hosts",
		"mytool --out=../x",
		"mytool --out ../x",
		"mytool OUT=/etc/x",
		"mytool .git",
		"mytool .GIT/hooks/pre-commit",
		"mytool .Git",
		"mytool ./.git/hooks/pre-commit",
		"mytool src/../.git/config",
		"mytool .claude/settings.json",
		"mytool src/link/target",
		"mytool src/gitlink/hooks/pre-commit",
		"mytool -- ../x",
		"go build -o /tmp/out ./...",
		"mytool ok.txt ../x",
		"FOO=1 mytool ../x",
		"mytool < in.txt ../x",
		"true; mytool ../x",
	}
	for _, command := range refused {
		d := gate.EvaluateCommand(command, nil)
		if d.Action != Deny || d.Rule != RuleCommandOperandUnverifiable || d.Severity != Hard || !IsUnreadableRule(d.Rule) {
			t.Errorf("%q = %s/%s/%s (%s); want hard %s", command, d.Action, d.Rule, d.Severity, d.Reason, RuleCommandOperandUnverifiable)
		}
	}

	untouched := []string{
		"go test ./...",
		"cargo build --release",
		"npm run build -- --watch",
		"pytest tests/ -k foo",
		"mytool src/main.go",
		"mytool --flag value",
		"mytool -o out.txt",
		"mytool https://example.com/a/../b",
		"mytool 'a note about ../x'",
		"mytool $HOME/x",
		"mytool --out=$HOME/x",
		"mytool .gitignore .github/workflows/ci.yml .gitattributes",
		"mytool /dev/null",
		"mytool src/not-a-link",
		"cat /etc/hosts",
		"ls ../x",
		"head -n 3 ../x",
		"rg pattern ../x",
		"grep -r foo ../x",
		"wc -l .git/config",
		"diff ../a ../b",
		"stat .git",
		"file ../x",
		"sort ../x",
		"uniq ../x",
		"cut -d, -f1 ../x",
		"echo ../x",
		"printf '%s' /etc/x",
		"git log -- ../x",
		"git status",
	}
	for _, command := range untouched {
		d := gate.EvaluateCommand(command, nil)
		if d.Rule == RuleCommandOperandUnverifiable {
			t.Errorf("%q refused as %s (%s); it names no protected path or is a modelled program", command, d.Rule, d.Reason)
		}
	}
	if d := (CommandGate{HardRules: false, Root: root}).EvaluateCommand("mytool ../x", nil); d.Rule == RuleCommandOperandUnverifiable {
		t.Errorf("with hard rules off the operand rung must not fire: %+v", d)
	}
	_ = strings.TrimSpace
}

// A path inside a JSON operand is handed to the program as surely as a bare
// one. `claude mcp add-json lp '{"command":"/opt/x"}'` passed: as one word the
// document read as a relative file name inside the root, and pretty-printed it
// had whitespace and read as prose.
func TestUnmodelledProgramJSONOperandsAreOpened(t *testing.T) {
	root := t.TempDir()
	if err := os.MkdirAll(filepath.Join(root, ".git", "hooks"), 0o755); err != nil {
		t.Fatal(err)
	}
	gate := CommandGate{HardRules: true, Root: root}

	// One past the string-count and depth bounds in command_operands.go (4096
	// strings, depth 64), written as numbers so the test builds against the code
	// before the fix and fails there on behaviour. Raising a bound fails it
	// loudly. Empty strings keep the document under fnmatch's 16384-unit cap,
	// past which the secret-read rung refuses the word first (see below).
	strs := make([]string, 4097)
	for i := range strs {
		strs[i] = `""`
	}
	refused := []string{
		`claude mcp add-json --scope user lp '{"type":"stdio","command":"/opt/x","args":["mcp"]}'`,
		`claude mcp add-json lp '{"type": "stdio", "command": "/opt/x"}'`,
		`mytool '["/etc/x"]'`,
		`mytool '[{"nested":{"deeper":["../x"]}}]'`,
		`mytool --json='{"p":"../x"}'`,
		`mytool CONFIG='{"p":"/etc/x"}'`,
		`mytool '{"args":["--out=/etc/x"]}'`,
		`mytool '{"p":".git/hooks/pre-commit"}'`,
		`mytool '{"/etc/x":1}'`,
		`mytool '{"config":"{\"command\":\"/etc/x\"}"}'`,
		`mytool ' {"p":"/etc/x"} '`,
		"mytool '" + strings.Repeat("[", 66) + strings.Repeat("]", 66) + "'",
		"mytool '[" + strings.Join(strs, ",") + "]'",
	}
	for _, command := range refused {
		d := gate.EvaluateCommand(command, nil)
		if d.Action != Deny || d.Rule != RuleCommandOperandUnverifiable || d.Severity != Hard {
			label := command
			if len(label) > 120 {
				label = label[:120] + "…"
			}
			t.Errorf("%q = %s/%s/%s (%s); want hard %s", label, d.Action, d.Rule, d.Severity, d.Reason, RuleCommandOperandUnverifiable)
		}
	}

	// A word past fnmatch's 16384-unit cap is refused by the secret-read rung,
	// which runs before this one, so through the gate the 64 KiB bound can only
	// be seen as a hard refusal by either rung. The bound itself is checked on
	// jsonOperandStrings, over it and exactly at it.
	over := `["` + strings.Repeat("a", maxJSONOperandBytes) + `"]`
	if d := gate.EvaluateCommand("mytool '"+over+"'", nil); d.Action != Deny || d.Severity != Hard {
		t.Errorf("a %d-byte JSON operand = %s/%s/%s; want a hard deny", len(over), d.Action, d.Rule, d.Severity)
	}
	if _, isJSON, ok := jsonOperandStrings(over); !isJSON || ok {
		t.Errorf("a %d-byte JSON document: isJSON %v ok %v; want isJSON and not ok", len(over), isJSON, ok)
	}
	atBound := `["` + strings.Repeat("a", maxJSONOperandBytes-4) + `"]`
	if strs, isJSON, ok := jsonOperandStrings(atBound); !isJSON || !ok || len(strs) != 1 {
		t.Errorf("a %d-byte JSON document: %d strings, isJSON %v ok %v; want one string, read in full", len(atBound), len(strs), isJSON, ok)
	}

	untouched := []string{
		`mytool '{"command":"bin/tool","args":["src/main.go","--out=dist/x"]}'`,
		`mytool '{"url":"https://example.com/a/../b"}'`,
		`mytool '{"note":"a note about ../x"}'`,
		`mytool '{"dev":"/dev/null","n":3,"ok":true,"none":null}'`,
		`mytool '{}'`,
		`mytool '[]'`,
		`mytool '{nope'`,
		`mytool '[not json'`,
		`cat '{"command":"/etc/x"}'`,
		`echo '{"command":"/etc/x"}'`,
	}
	for _, command := range untouched {
		d := gate.EvaluateCommand(command, nil)
		if d.Rule == RuleCommandOperandUnverifiable {
			t.Errorf("%q refused as %s (%s); it names no protected path or is a modelled program", command, d.Rule, d.Reason)
		}
	}
	if d := (CommandGate{HardRules: false, Root: root}).EvaluateCommand(`mytool '{"p":"/etc/x"}'`, nil); d.Rule == RuleCommandOperandUnverifiable {
		t.Errorf("with hard rules off the operand rung must not fire: %+v", d)
	}
	// Keys are read in sorted order, so the path a refusal names does not
	// depend on map iteration order.
	for range 20 {
		if d := gate.EvaluateCommand(`mytool '{"b":"/two","a":"/one"}'`, nil); !strings.Contains(d.Reason, "`/one`") {
			t.Fatalf("refusal names the wrong path, or none: %s/%s (%s)", d.Action, d.Rule, d.Reason)
		}
	}
}

// sort -o and uniq's second operand write; the reads-only table does not exempt them.
func TestReadsOnlyProgramsStillHaveTheirWritesJudged(t *testing.T) {
	for line, want := range map[string][]string{
		"sort -o .env in.txt":       {".env"},
		"sort --output=.env in.txt": {".env"},
		"sort -o out in.txt":        {"out"},
		"sort in.txt":               nil,
		"uniq in.txt .env":          {".env"},
		"uniq in.txt":               nil,
		"uniq -c in.txt out.txt":    {"out.txt"},
	} {
		got := argumentWriteTargets(line).targets
		if strings.Join(got, ",") != strings.Join(want, ",") {
			t.Errorf("%q: targets %q, want %q", line, got, want)
		}
	}
}
