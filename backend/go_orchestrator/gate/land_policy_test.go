package gate

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/flags"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/policy"
)

func landGate(t *testing.T, posture string) (*Gate, string) {
	mainRoot, _, _ := setupGateWorktreeEnv(t)
	must := func(err error) {
		if err != nil {
			t.Fatal(err)
		}
	}
	must(os.MkdirAll(filepath.Join(mainRoot, "src"), 0o755))
	must(os.WriteFile(filepath.Join(mainRoot, ".env"), []byte("SECRET=1\n"), 0o644))
	must(os.WriteFile(filepath.Join(mainRoot, "README.md"), []byte("hi\n"), 0o644))
	must(os.Link(filepath.Join(mainRoot, ".env"), filepath.Join(mainRoot, "src", "hl")))
	must(os.Symlink("../.env", filepath.Join(mainRoot, "src", "sl")))
	must(os.WriteFile(filepath.Join(mainRoot, "list.txt"), []byte("src\n"), 0o644))
	return newGateAtRoot(t, mainRoot, map[string]string{flags.HarnessPosture: posture}), mainRoot
}

// Lane W regression: a wrapper's wrapped program may be followed by operands
// that name a reader later in the line; every word after the wrapper is judged.
func TestLandWWrapperSkipsWrappedProgramOperands(t *testing.T) {
	for _, posture := range []string{flags.PostureStrict, flags.PostureDev} {
		g, _ := landGate(t, posture)
		for _, c := range []string{"nice awk 1 .env cat", "timeout 5 awk 1 .env cat", "nice -n 5 awk 1 .env head", "xargs -a .env cat", "xargs --arg-file=.env cat", "stdbuf -oL mytool .env cat README.md"} {
			for _, withTask := range []bool{true, false} {
				var task *dc.Task
				if withTask {
					task = testTask()
					task.AllowedCommands = []string{"nice *", "timeout *", "xargs *", "stdbuf *"}
				}
				d, err := g.EvaluateCommand(c, task)
				if err != nil {
					t.Fatal(err)
				}
				if d.Rule != policy.RuleCommandSecretRead {
					t.Errorf("%s task=%v %q: got %s %v %q, want command.secret_read (base)", posture, withTask, c, d.Action, d.Severity, d.Rule)
				}
			}
		}
	}
}

// Lane V: a line past the binding caps must still report what it does
// (decision.go unreadableRules invariant), and say why it was refused.
func TestLandVTruncationKeepsWhatItDoes(t *testing.T) {
	g, _ := landGate(t, flags.PostureStrict)
	task := testTask()
	task.AllowedCommands = []string{"git push *", "cat *", "echo *"}
	loop := "; for f in a b c d e f g h i j k l m n o p q; do echo $f; done"
	for c, want := range map[string]policy.RuleID{
		"git push --force origin main" + loop: policy.RuleCommandForcePush,
		"cat .env" + loop:                     policy.RuleCommandSecretRead,
		"for f in .env a b c d e f g h i j k l m n o p q; do cat $f; done": policy.RuleCommandSecretRead,
	} {
		d, err := g.EvaluateCommand(c, task)
		if err != nil {
			t.Fatal(err)
		}
		if d.Rule != want {
			t.Errorf("%q: got %s %v %q, want %q", c, d.Action, d.Severity, d.Rule, want)
		}
	}
	// With nothing hard to report the truncation is the refusal, named for what it is.
	d, err := g.EvaluateCommand("echo hi"+loop, task)
	if err != nil {
		t.Fatal(err)
	}
	if !d.Blocked() || !strings.Contains(d.Reason, "more values than the gate judges") || strings.Contains(d.Reason, "chosen by a command substitution") {
		t.Errorf("plain truncated loop: got %s %q %q, want a refusal that says the loop passed the binding caps", d.Action, d.Rule, d.Reason)
	}
}

// Lane W false refusals: a command that reads no file, a computed number, and
// the working directory are not unresolvable file operands.
func TestLandControlRowsStayAllowed(t *testing.T) {
	for _, posture := range []string{flags.PostureStrict, flags.PostureDev} {
		g, _ := landGate(t, posture)
		task := testTask()
		task.AllowedCommands = []string{"exit *", "kill *", "wait *", "sleep *", "seq *", "make *", "go test *",
			"npm run *", "head *", "cat *", "git log *", "git show *", "git diff *"}
		for _, c := range []string{
			"exit $?", "kill $!", "wait $!", "sleep ${D:-1}", "seq 1 ${N:-3}",
			"make -j\"${N:-4}\"", "go test -count=\"${COUNT:-1}\" ./...",
			"head -n \"${N:-5}\" README.md", "git log -n \"${N:-5}\"", "git log --max-count=\"${N:-5}\"",
			"head -n \"$N\" README.md",
		} {
			d, err := g.EvaluateCommand(c, task)
			if err != nil {
				t.Fatal(err)
			}
			if d.Blocked() {
				t.Errorf("%s %q: control row refused: %s %q (%s)", posture, c, d.Action, d.Rule, d.Reason)
			}
		}
	}
}

// Every refusal that the control rows sit beside stays: a computed value in a
// file position, an unknown directory, and a computed git operand.
func TestLandNarrowedRowsStillRefuse(t *testing.T) {
	for _, posture := range []string{flags.PostureStrict, flags.PostureDev} {
		g, _ := landGate(t, posture)
		task := testTask()
		task.AllowedCommands = []string{"make *", "head *", "cat *", "git log *", "git show *", "git diff *", "seq *"}
		for _, c := range []string{
			"cat $OLDPWD/README.md", "cat \"$PWD/.env\"", "cat ${PWD}/.env", "cat $zz", "cat ${zz:-.env}",
			"cat -n ${F:-.env}", "head -n ${N:-5} ${F:-.env}", "head ${N:-.env} README.md",
			"make ${T:-all}", "git log ${B:-.env}", "git show ${zz:-HEAD:.env}", "git log -n ${N:-5} ${F:-.env}",
			"git diff --$X", "seq 1 3 > .env", "cat .env; exit $?",
		} {
			d, err := g.EvaluateCommand(c, task)
			if err != nil {
				t.Fatal(err)
			}
			if !d.Blocked() || d.Severity != policy.Hard {
				t.Errorf("LEAK %s %q: want hard refusal, got %s %q", posture, c, d.Action, d.Rule)
			}
		}
		// A computed git operand keeps the train's hard, unsoftenable rule.
		d, err := g.EvaluateCommand("git log ${B:-main}", task)
		if err != nil {
			t.Fatal(err)
		}
		if d.Rule != policy.RuleCommandGitReadArgument {
			t.Errorf("%s computed git operand: got %q, want %q", posture, d.Rule, policy.RuleCommandGitReadArgument)
		}
	}
}

// Fix round: options that take an optional or no value, end of options, other
// programs, $PWD rebinding and file readers keep refusing. A row in gitHard was
// a hard refusal under a rule a host cannot soften on the train (3b89faeb); it
// must not become a softenable (unreadable) one.
func TestLandFixRoundRowsRefuse(t *testing.T) {
	gitHard := map[string]bool{
		"git show -U ${x:-HEAD:.env}": true, "git show --unified ${x:-HEAD:.env}": true,
		"git show --abbrev ${x:-HEAD:.env}": true, "git diff -U ${x:-.env}": true,
		"git log -p -U ${x:-.env}": true, "git log -p -- -n ${x:-.env}": true,
		"git diff -- -U ${x:-.env}": true, "git show -- -U ${x:-HEAD:.env}": true,
	}
	rows := []string{
		"git blame -n ${x:-.env}", "head -- -n ${x:-.env}", "grep -e S -- --count ${x:-.env}",
		"uniq --count ${x:-.env}", "grep -e SECRET --count ${x:-.env}", "rg -e . --count ${x:-.env}",
		"sed -n p --depth ${x:-.env}", "mytool --port ${x:-.git/hooks/pre-commit}",
		"mytool --depth ${x:-../outside}", "npm run dev -- --port ${PORT:-3000}",
		"PWD=$(echo src); cat $PWD/sl", "printf -v PWD src; cat $PWD/sl", "read PWD < list.txt; cat $PWD/sl",
		"export PWD=$(echo src); cat $PWD/sl", "declare PWD=$(echo src); cat $PWD/sl", "unset PWD; cat $PWD/x",
		"cat $PWD/src/sl", "date -f ${x:-.env}", "date --file=${x:-.env}", "hostname -F ${x:-.env}",
	}
	for c := range gitHard {
		rows = append(rows, c)
	}
	for _, posture := range []string{flags.PostureStrict, flags.PostureDev} {
		g, _ := landGate(t, posture)
		task := testTask()
		task.AllowedCommands = []string{"*"}
		for _, c := range rows {
			d, err := g.EvaluateCommand(c, task)
			if err != nil {
				t.Fatal(err)
			}
			if !d.Blocked() || d.Severity != policy.Hard {
				t.Errorf("LEAK %s %q: want hard refusal, got %s %q", posture, c, d.Action, d.Rule)
			}
			if gitHard[c] && policy.IsUnreadableRule(d.Rule) {
				t.Errorf("%s %q: train refused this under a non-softenable rule, got unreadable %q", posture, c, d.Rule)
			}
		}
		// The gate root is not the process working directory, and $PWD is
		// unbound: cat $PWD/src/sl (src/sl is a symlink to .env) is in rows.
	}
}

// Round 3: an unquoted computed value word-splits (head -n ${x:-1 .env} reads
// .env), so it is exempt in no form; only an expansion wholly inside double
// quotes is. $PWD is unbound as on the train. A gitHard row was a hard,
// unsoftenable refusal on the train and must stay one.
func TestLandRound3RowsRefuse(t *testing.T) {
	gitHard := map[string]bool{
		"git log -p -n ${x:-1 .env}": true, "git log -p --max-count=${x:-1 .env}": true,
		"git log -p -U${x:-0 .env}": true, "git log -n ${N:-5}": true,
		"git log --max-count=${N:-5}": true, "git log -n${N:-5}": true,
	}
	rows := []string{
		"head -n ${x:-1 .env}", "N=$(echo 1 .env); head -n $N README.md", "read N < list1.txt; head -n $N README.md",
		"head -n $N README.md", "head -n ${N:-5} README.md", "tail -c ${x:-1 .env}", "head -n ${x:-1 .e*}",
		"pytest -n ${x:-1 .env}", "make -j ${x:-4 -f.env}", "head -n${x:-1 .env}", "make -j${N:-4}",
		"go test -count=${COUNT:-1} ./...", "cat \"$PWD/README.md\"", "cat ${PWD}/README.md", "cat $PWD/src/sl",
	}
	for c := range gitHard {
		rows = append(rows, c)
	}
	for _, posture := range []string{flags.PostureStrict, flags.PostureDev} {
		g, _ := landGate(t, posture)
		task := testTask()
		task.AllowedCommands = []string{"*"}
		for _, c := range rows {
			d, err := g.EvaluateCommand(c, task)
			if err != nil {
				t.Fatal(err)
			}
			if !d.Blocked() || d.Severity != policy.Hard {
				t.Errorf("LEAK %s %q: want hard refusal, got %s %q", posture, c, d.Action, d.Rule)
			}
			if gitHard[c] && d.Rule != policy.RuleCommandGitReadArgument {
				t.Errorf("%s %q: want %s as on the train, got %q", posture, c, policy.RuleCommandGitReadArgument, d.Rule)
			}
		}
	}
}
