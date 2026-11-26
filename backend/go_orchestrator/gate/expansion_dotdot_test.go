package gate

import (
	"os"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/flags"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/policy"
)

// ft-463dbc1d: filepath.Clean treats an expansion as a directory a later ".."
// cancels, but the shell expands an unset variable to nothing, so the write
// lands somewhere else. Every leak row must be refused hard under every
// posture and task shape, by one of the rules the row names; every control row
// must be allowed exactly as it was before the change.

const (
	ruleOutside      = policy.RuleOutsideRoot
	ruleUnverifiable = policy.RuleID("command.operand.unverifiable")
	ruleGitRead      = policy.RuleID("command.git_read_argument")
)

type expansionRow struct {
	cmd   string
	rules []policy.RuleID
}

func outside(cmd string) expansionRow { return expansionRow{cmd, []policy.RuleID{ruleOutside}} }

func writeRow(cmd string) expansionRow {
	return expansionRow{cmd, []policy.RuleID{ruleOutside, ruleUnverifiable}}
}

func operandRow(cmd string) expansionRow {
	return expansionRow{cmd, []policy.RuleID{ruleOutside, ruleUnverifiable}}
}

var expansionDotDotLeaks = []expansionRow{
	// The card.
	outside("echo x > src/$zz/../Makefile"),
	outside("echo x >> src/$zz/../Makefile"),
	outside("echo x > src/$a/$b/$c/../../../outside.txt"),
	outside("echo x > src/${zz:-..}/../x.go"),
	outside("echo x > {src,x}/../Makefile"),
	outside("echo x > ~root/../Makefile"),
	outside("echo x > ${zz:-..}/Makefile"),
	outside("echo x > src/$a/../.git/hooks/pre-commit"),
	outside("echo x | tee src/$zz/../Makefile"),
	outside("cp src/calc.go src/$zz/../Makefile"),
	outside("mv src/calc.go src/$zz/../Makefile"),
	outside("rm src/$zz/../README.md"),
	writeRow("dd if=src/calc.go of=src/$zz/../Makefile"),
	outside("sed -i s/a/b/ src/$zz/../Makefile"),
	outside("ln -s x src/$zz/../Makefile"),
	// ANSI-C escapes and operator concatenation spell the ".." (F1).
	outside("echo x > src/$'\\x2e\\x2e'/$'\\x2e\\x2e'/o.txt"),
	outside("echo x > src/$'\\x2e\\x2e'/.git/hooks/pre-commit"),
	outside("echo x > src/$'\\56\\56'/$'\\56\\56'/o.txt"),
	outside("echo x > src/${zz:-.}${zz:-.}/${zz:-.}${zz:-.}/o.txt"),
	outside("cp src/calc.go src/$'\\x2e\\x2e'/$'\\x2e\\x2e'/o.txt"),
	outside("cp src/calc.go src/${zz:-.}${zz:-.}/${zz:-.}${zz:-.}/o.txt"),
	outside("echo x | tee src/$'\\x2e\\x2e'/$'\\x2e\\x2e'/o.txt"),
	outside("echo x | tee src/${zz:-.}${zz:-.}/${zz:-.}${zz:-.}/o.txt"),
	// An unmodelled program is handed such a path (F1b).
	{"mytool src/$zz/../.env", []policy.RuleID{"command.secret_read"}},
	operandRow("mytool src/.$zz./.git/config"),
	operandRow("mytool src/$zz../.git/config"),
	operandRow("mytool src/.$zz./.$zz./o.txt"),
	operandRow("mytool --out=src/$zz/../.git/config"),
	// Right to refuse, and under a path rule (F3b), never secret_read.
	operandRow("make -C $dir/.."),
	writeRow("cp -t src/$zz/../.. src/calc.go"),
	writeRow("mv -t src/$zz/../.. src/calc.go"),
	writeRow("cat <> src/$zz/../../o.txt"),
	operandRow("tar -C src/$zz/../.. -xf a.tar"),
	operandRow("unzip -d src/$zz/../.. a.zip"),
	operandRow("go build -o src/$zz/../../o.txt ./..."),
	// A git read's pathspec (Fable ruling Q2 (i)).
	{"git log -- src/$zz/../README.md", []policy.RuleID{ruleGitRead}},
	{"git show HEAD:src/$zz/../README.md", []policy.RuleID{ruleGitRead}},
	// A binding the line does not have in effect (F2).
	outside("echo x > src/$d/../../o.txt"),
	outside("d=lib; unset d; echo x > src/$d/../../o.txt"),
	outside("d=lib; d=$zz; echo x > src/$d/../../o.txt"),
	outside("d=lib; read d < f; echo x > src/$d/../x"),
	outside("echo x > src/$d/../../o.txt; d=lib"),
}

// Controls: judged as before the change. The first group is allowed outright;
// the rest are the words F3 over-refused (URLs, prose, single-quoted literals,
// JSON, revision ranges).
var expansionDotDotControls = []string{
	"echo x > src/calc.go",
	"d=lib; echo x > src/$d/../calc.go",
	"d=src; echo x > $d/calc.go",
	"curl -s https://example.com/$v/../api",
	"mytool https://example.com/${v}/../api",
	"gh pr create --body \"paths like $d/../x\"",
	"node -e \"require('$d/../x')\"",
	"cat 'src/$x/../README.md'",
	"jq . '{\"a\":\"$x/../y\"}'",
	"mytool 'src/$x/../y'",
	"echo x > 'src/$zz/../calc.go'",
	"git log --oneline main..$b",
	"git diff $a..$b",
	"git log $a...$b",
}

func ruleIn(rule policy.RuleID, rules []policy.RuleID) bool {
	for _, r := range rules {
		if r == rule {
			return true
		}
	}
	return false
}

func TestExpansionBeforeDotDotRefusedHard(t *testing.T) {
	mainRoot, _, _ := setupGateWorktreeEnv(t)
	os.MkdirAll(mainRoot+"/src", 0o755)
	for _, posture := range []string{flags.PostureStrict, flags.PostureDev} {
		g := newGateAtRoot(t, mainRoot, map[string]string{flags.HarnessPosture: posture})
		starTask := func() *dc.Task {
			task := testTask()
			task.PlannedFiles = append(task.PlannedFiles, dc.PlannedFile{Path: "src/**", AllowedChange: dc.ChangeModify})
			task.AllowedCommands = []string{"*"}
			return task
		}
		for name, task := range map[string]*dc.Task{"star": starTask(), "none": nil} {
			for _, row := range expansionDotDotLeaks {
				d, err := g.EvaluateCommand(row.cmd, task)
				if err != nil {
					t.Fatal(err)
				}
				if !d.Blocked() || d.Severity != policy.Hard || !ruleIn(d.Rule, row.rules) {
					t.Errorf("%s/%s %q: got %s %v %q, want hard %v", posture, name, row.cmd, d.Action, d.Severity, d.Rule, row.rules)
				}
			}
		}
		for _, cmd := range expansionDotDotControls {
			d, err := g.EvaluateCommand(cmd, starTask())
			if err != nil {
				t.Fatal(err)
			}
			if d.Blocked() {
				t.Errorf("%s control %q: blocked (%s %v %q)", posture, cmd, d.Action, d.Severity, d.Rule)
			}
		}
	}
}

// In dev the train already refused these as hard operand.unverifiable (the
// unmodelled-operand rung); the change must keep that rule, not replace it.
func TestExpansionBeforeDotDotDevKeepsUnverifiableRule(t *testing.T) {
	mainRoot, _, _ := setupGateWorktreeEnv(t)
	g := newGateAtRoot(t, mainRoot, map[string]string{flags.HarnessPosture: flags.PostureDev})
	for _, cmd := range []string{
		"cp -t src/$zz/../.. src/calc.go",
		"dd if=src/calc.go of=src/$zz/../../o.txt",
		"cat <> src/$zz/../../o.txt",
	} {
		task := testTask()
		task.AllowedCommands = []string{"*"}
		d, err := g.EvaluateCommand(cmd, task)
		if err != nil {
			t.Fatal(err)
		}
		if d.Severity != policy.Hard || d.Rule != ruleUnverifiable {
			t.Errorf("dev %q: got %s %v %q, want hard %q", cmd, d.Action, d.Severity, d.Rule, ruleUnverifiable)
		}
	}
}
