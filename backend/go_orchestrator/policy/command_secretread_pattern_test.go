package policy

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
)

// patternReadRoot is a tree that holds `.env.local`, which is what turned the
// regex `.*` into a credential read: expanded as a wildcard against the tree,
// `.*` matches it.
func patternReadRoot(t *testing.T) string {
	t.Helper()
	root := secretReadRoot(t)
	for name, body := range map[string]string{
		".env.local":   "TOKEN=x",
		"data.json":    `{"env":"prod"}`,
		"patterns.txt": "foo\n",
	} {
		if err := os.WriteFile(filepath.Join(root, name), []byte(body), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	return root
}

// The first operand of a search or text tool is the program it runs — a regex,
// a sed script, an awk program, a jq filter — not a file it reads. Expanding it
// as a wildcard against the tree refused `grep -o 'gh:.*'` as a read of
// `.env.local` (GitPulse hook, 2026-11-24).
func TestSecretReadDoesNotJudgeAProgramOperandAsAPath(t *testing.T) {
	root := patternReadRoot(t)
	gate := CommandGate{HardRules: true, Root: root, GlobalAllowedCommands: []string{"*"}}

	for _, command := range []string{
		`gh api users/x --jq '"gh:" + .type' | grep -o 'gh:.*'`,
		`grep -o 'gh:.*' notes.txt`,
		`grep '.*' README.md`,
		`grep -n -i -- '.*' README.md`,
		`grep -A 3 'x.*' README.md`,
		`grep -A3 'x.*' README.md`,
		`grep -m 1 -C2 '.*' README.md`,
		`grep -rn --include='*.go' 'a:.*' src`,
		`grep -e '.*' README.md`,
		`grep --regexp='.*' README.md`,
		`grep -e foo -e '.*' README.md`,
		`egrep '.*' README.md`,
		`fgrep '.*' README.md`,
		`rg -n 'a:.*' src`,
		`rg -t go '.*' src`,
		`rg --max-count 2 '.*' README.md`,
		`rg -e '.*' src`,
		`ag '.*' src`,
		`ag -G go '.*' src`,
		`sed 's/:.*//' README.md`,
		`sed -n -e 's/:.*//p' README.md`,
		`sed -E 's/a:.*//' README.md`,
		`awk '/a:.*/' README.md`,
		`awk -F: '{print $1}' README.md`,
		`awk -v k=v '/a:.*/' README.md`,
		`jq -r .env data.json`,
		`jq --arg n x '.env' data.json`,
		`cat README.md | grep -o 'gh:.*'`,
	} {
		d := gate.EvaluateCommand(command, nil)
		if d.Rule == RuleCommandSecretRead {
			t.Errorf("%q was refused as a secret read: %s", command, d.Reason)
		}
	}
}

// What the program operand is not: the files the tool reads. Every one of these
// still names a credential file as something to read.
func TestSecretReadStillJudgesTheFilesASearchToolReads(t *testing.T) {
	root := patternReadRoot(t)
	gate := CommandGate{HardRules: true, Root: root}

	for _, command := range []string{
		`cat .env.local`,
		`grep x .env*`,
		`grep x .env.local`,
		`grep -o 'gh:.*' .env.local`,
		`grep -e x .env.local`,
		`grep -e x -- .env.local`,
		`grep -A 3 x .env.local`,
		`grep -- -x .env.local`,
		`grep -f .env.local README.md`,
		`grep -f.env.local README.md`,
		`grep --file .env.local README.md`,
		`grep --file=.env.local README.md`,
		`grep -rn token --include='.env*' .`,
		`egrep '.*' .env.local`,
		`rg secret -g '.env*'`,
		`rg secret -g.env*`,
		`rg -g '.env*' secret`,
		`rg --glob=.env* secret`,
		`rg -e x .env.local`,
		`rg -f .env.local README.md`,
		`ag x .env.local`,
		`sed -n p .env.local`,
		`sed 's/:.*//' .env.local`,
		`sed -e p .env.local`,
		`sed -f .env.local README.md`,
		`awk 1 .env.local`,
		`awk -f .env.local README.md`,
		`jq . .env.local`,
		`jq -r .env .env.local`,
		`jq -f .env.local data.json`,
		`jq -n --rawfile k .env.local '$k'`,
		`jq -n --slurpfile k .env.local '$k'`,
	} {
		d := gate.EvaluateCommand(command, nil)
		if d.Action != Deny || d.Severity != Hard || d.Rule != RuleCommandSecretRead {
			t.Errorf("%q: %s %s/%s (%s), want a hard %s", command, d.Action, d.Rule, d.Severity, d.Reason, RuleCommandSecretRead)
		}
	}
}

// The reported command is allowed outright where the allowlist admits it: no
// other rung refuses the same word under another name.
func TestReportedPatternPipelineIsAllowed(t *testing.T) {
	root := patternReadRoot(t)
	gate := CommandGate{HardRules: true, Root: root}
	task := &dc.Task{ID: "TASK-001", AllowedCommands: []string{"gh *", "grep *"}}
	command := `gh api users/x --jq '"gh:" + .type' | grep -o 'gh:.*'`
	if d := gate.EvaluateCommand(command, task); d.Action != Allow {
		t.Fatalf("%q: %s %s/%s (%s), want allow", command, d.Action, d.Rule, d.Severity, d.Reason)
	}
}

// Every flag in a tool's table is a flag: were one misread as taking a value,
// the pattern would be consumed as that value and the credential file after
// it taken for the program. Each spelling below must still be refused.
func TestSecretReadProgramToolFlagsDoNotHideTheFileOperand(t *testing.T) {
	root := patternReadRoot(t)
	gate := CommandGate{HardRules: true, Root: root}
	tools := map[string]programTool{"git grep": gitGrepTool}
	for name, tool := range programTools {
		tools[name] = tool
	}
	for name, tool := range tools {
		program := "x"
		if name == "jq" {
			program = "."
		}
		var spellings []string
		for _, letter := range tool.shortFlag {
			spellings = append(spellings, "-"+string(letter))
		}
		for _, long := range tool.longFlag {
			spellings = append(spellings, "--"+long)
		}
		for _, flag := range spellings {
			if name == "sed" && (flag == "-i" || flag == "-I" || flag == "--in-place") {
				// In-place makes every file operand a write target, which the
				// write rungs judge instead (argumentWriteTargetsInRoot).
				continue
			}
			command := name + " " + flag + " " + program + " .env.local"
			if name == "git grep" {
				command = name + " " + flag + " " + program + " -- .env.local"
			}
			d := gate.EvaluateCommand(command, nil)
			if d.Rule != RuleCommandSecretRead {
				t.Errorf("%q: %s %s/%s (%s), want %s", command, d.Action, d.Rule, d.Severity, d.Reason, RuleCommandSecretRead)
			}
		}
	}
}

// The file operands are files wherever they stand, and an option the table
// does not know leaves the clause judged as before: it may have taken the
// next word as its value.
func TestSecretReadProgramOperandPermutationsAndUnknownOptions(t *testing.T) {
	root := patternReadRoot(t)
	gate := CommandGate{HardRules: true, Root: root}
	for _, command := range []string{
		`grep .env.local -e x`,
		`grep -f patterns.txt .env.local`,
		`awk -f patterns.txt .env.local`,
		`jq -f patterns.txt .env.local`,
		`sed -f patterns.txt .env.local`,
		`rg --files .env.local`,
		`grep --label foo .env.local README.md`,
		`awk --file .env.local README.md`,
		`grep -Q .env.local README.md`,
		`git grep API_KEY -- .env.local`,
		`git grep -e x HEAD -- .env.local`,
		// An option the table does not know, anywhere, exempts nothing: GNU
		// grep and sed permute arguments and take abbreviations, so --reg is
		// --regexp and .env.local is a file.
		`grep .env.local --reg TOKEN`,
		`grep .env.local --regex=TOKEN`,
		`sed .env.local --expr p`,
		// A word the shell expands is file names, not a program.
		`grep .* README.md`,
		`grep .env* README.md`,
		`grep -e .env* README.md`,
		// rg's config file and GREP_OPTIONS can supply the pattern, so a first
		// operand that spells a credential path may be read as one.
		`rg .env.local`,
		`rg -- .env.local`,
		`RIPGREP_CONFIG_PATH=/tmp/c rg .env.local`,
		`export RIPGREP_CONFIG_PATH=/tmp/c; rg .env.local`,
		`GREP_OPTIONS='-e TOKEN' grep .env.local`,
		`env GREP_OPTIONS=-eX grep .env.local`,
		`grep .env.local`,
		// The same holds for a bare credential name searched as a regex: main
		// refused it too, and GREP_OPTIONS='-e x' would make it the file read.
		`grep .env .gitignore`,
	} {
		d := gate.EvaluateCommand(command, nil)
		if d.Action != Deny || d.Severity != Hard || d.Rule != RuleCommandSecretRead {
			t.Errorf("%q: %s %s/%s (%s), want a hard %s", command, d.Action, d.Rule, d.Severity, d.Reason, RuleCommandSecretRead)
		}
	}
	allowAll := CommandGate{HardRules: true, Root: root, GlobalAllowedCommands: []string{"*"}}
	for _, command := range []string{
		`git grep -n 'a:.*'`,
		`git grep -n 'a:.*' -- src`,
		`git -C . grep -e '.*' -- src`,
		// With --no-config rg reads no options from elsewhere, so its first
		// operand is the pattern.
		`rg --no-config .env.local src`,
		// Judged literally, a quoted regex is not a path.
		`rg -n 'a:.*' src`,
		`grep -o 'gh:.*' notes.txt`,
	} {
		if d := allowAll.EvaluateCommand(command, nil); d.Rule == RuleCommandSecretRead {
			t.Errorf("%q was refused as a secret read: %s", command, d.Reason)
		}
	}
}
