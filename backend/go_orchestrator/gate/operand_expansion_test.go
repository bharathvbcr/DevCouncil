package gate

import (
	"os"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/flags"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/policy"
)

// An operand the shell expands to a value the gate cannot name — a
// parameter-expansion operator, a brace expansion, a variable nothing binds —
// is never a clean allow (ft-ee1b04e7). Each row would read .env once sh
// expanded it, so each must be a hard refusal in every posture and with or
// without a task.
var operandExpansionLeaks = []string{
	"cat ${zz:-.env}",
	"for f in a; do cat ${f/a/.env}; done",
	"for f in a; do cat ${f%a}.env; done",
	"for f in a; do cat ${f:-.env}; done",
	"cat ${!zz}",
	"cat {.env,x}",
	"cat x{,.env}",
	"cat {a..c}",
	"cat $zz",
	"cat \"$zz\"",
	"cat --file=$zz",
	"git diff ${zz:-.env}",
	"for f in a; do git diff ${f/a/.env}; done",
	"git diff {.env,x}",
	"git show ${zz:-HEAD:.env}",
	"git diff HEAD -- $1",
	"git diff ${zz#a}",
	"git show $@",
	"grep x ${zz:-.env}",
	"grep x {.env,x}",
}

func operandExpansionGate(t *testing.T, posture string) *Gate {
	mainRoot, _, _ := setupGateWorktreeEnv(t)
	if err := os.WriteFile(mainRoot+"/.env", []byte("SECRET=1\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	return newGateAtRoot(t, mainRoot, map[string]string{flags.HarnessPosture: posture})
}

func TestOperandExpansionIsNeverACleanAllow(t *testing.T) {
	for _, posture := range []string{flags.PostureDev, flags.PostureStrict} {
		g := operandExpansionGate(t, posture)
		withTask := testTask()
		withTask.AllowedCommands = []string{"cat *", "git diff *", "git show *"}
		for name, task := range map[string]*dc.Task{"task": withTask, "no task": nil} {
			for _, c := range operandExpansionLeaks {
				d, err := g.EvaluateCommand(c, task)
				if err != nil {
					t.Fatalf("%s/%s %q: %v", posture, name, c, err)
				}
				if !d.Blocked() || d.Severity != policy.Hard {
					t.Errorf("LEAK %s/%s %q: want hard refusal, got action=%s rule=%q severity=%v", posture, name, c, d.Action, d.Rule, d.Severity)
				}
			}
		}
	}
}

// Control rows: a variable the line binds, with no operator, is still judged
// as the files it names and allowed.
func TestOperandExpansionControlRowsStayAllowed(t *testing.T) {
	for _, posture := range []string{flags.PostureDev, flags.PostureStrict} {
		g := operandExpansionGate(t, posture)
		task := testTask()
		task.AllowedCommands = []string{"cat *", "git diff *"}
		for _, c := range []string{
			"for f in a b; do cat $f; done",
			"for f in a b; do cat \"$f\"; done",
			"cat 'a$b'",
			"cat ${HOME}/x",
			"git diff HEAD~1 -- README.md",
			"git diff HEAD@{1}",
		} {
			d, err := g.EvaluateCommand(c, task)
			if err != nil {
				t.Fatalf("%s %q: %v", posture, c, err)
			}
			if d.Blocked() {
				t.Errorf("CONTROL %s %q: want allowed, got action=%s rule=%q reason=%s", posture, c, d.Action, d.Rule, d.Reason)
			}
		}
	}
}

// Fix round 1 (ft-ee1b04e7), each clause of the review. A row is a hard
// refusal in dev and strict; wantRule, when set, is the exact rule and must not
// be one a host may soften (policy.IsUnreadableRule).
func TestOperandExpansionReviewRound(t *testing.T) {
	type row struct {
		cmd, allow string
		noTask     bool
		wantRule   policy.RuleID
	}
	rows := []row{
		// 1: an unreadable operand must not demote a hard refusal beside it.
		{"sort ${zz:-y} $(devcouncil review record --file r.json)", "sort *", false, policy.RuleCommandHarnessOnly},
		{"git grep x ${zz:-y} $(devcouncil review record)", "git grep *", false, policy.RuleCommandHarnessOnly},
		{"git blame ${zz:-y} $(devcouncil review record)", "git blame *", false, policy.RuleCommandHarnessOnly},
		{"sort ${zz:-y} $(cat .e*)", "sort *", false, policy.RuleCommandSecretRead},
		// 2: $_ and the other special parameters.
		{"echo .env; git diff $_", "", true, ""},
		{"echo .env; git show HEAD:$_", "git show *", false, ""},
		{"echo .env; sort $_", "sort *", false, ""},
		{"git diff ${_}", "", true, ""},
		{"git diff $0", "", true, ""},
		{"git diff $$", "", true, ""},
		// 3: programs the gate does not model, and reading programs outside the table.
		{"awk 1 ${zz:-.env}", "awk *", false, ""},
		{"awk 1 {.env,x}", "awk *", false, ""},
		{"yq . ${zz:-.env}", "yq *", false, ""},
		{"perl -ne print ${zz:-.env}", "perl *", false, ""},
		{"sha256sum ${zz:-.env}", "sha256sum *", false, ""},
		{"column ${zz:-.env}", "column *", false, ""},
		{"mytool ${zz:-../x}", "mytool *", false, ""},
		{"mytool {x,.git/hooks/pre-commit}", "mytool *", false, ""},
		// 4: wrappers.
		{"nice cat ${zz:-.env}", "nice *", false, ""},
		{"caffeinate cat ${zz:-.env}", "caffeinate *", false, ""},
		{"nice -n 5 cat $zz", "nice *", false, ""},
		{"timeout 5 cat {.env,x}", "timeout *", false, ""},
		{"sudo -u x cat $zz", "sudo *", false, ""},
		// 5: input redirection.
		{"cat < ${zz:-.env}", "cat *", false, ""},
		{"< ${zz:-.env} cat", "cat *", false, ""},
		{"cat < $zz", "cat *", false, ""},
		{"tr a b < ${zz:-.env}", "tr *", false, ""},
		// 6: ANSI-C quoting.
		{"cat $'.env'", "cat *", false, ""},
		{"cat $'\\x2eenv'", "cat *", false, ""},
		{"git show HEAD:$'.env'", "git show *", false, ""},
		{"grep x $'.env'", "grep *", false, ""},
	}
	for _, posture := range []string{flags.PostureDev, flags.PostureStrict} {
		g := operandExpansionGate(t, posture)
		for _, r := range rows {
			var task *dc.Task
			if !r.noTask {
				task = testTask()
				task.AllowedCommands = []string{r.allow, "devcouncil *"}
			}
			d, err := g.EvaluateCommand(r.cmd, task)
			if err != nil {
				t.Fatalf("%s %q: %v", posture, r.cmd, err)
			}
			bad := !d.Blocked() || d.Severity != policy.Hard
			if r.wantRule != "" {
				bad = d.Rule != r.wantRule || policy.IsUnreadableRule(d.Rule)
			}
			if bad {
				t.Errorf("LEAK %s %q: got action=%s rule=%q severity=%v", posture, r.cmd, d.Action, d.Rule, d.Severity)
			}
		}
	}
}
