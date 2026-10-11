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
