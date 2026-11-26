package gate

import (
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/flags"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/policy"
)

// A spelled credential keeps its hard refusal when the line also makes the
// variable naming it unbindable (export, unset, a later non-literal value, a
// read before the assignment), and when env -S hides the command: the clause
// as written is still judged, redirections and later words included.
func TestSpelledCredentialSurvivesUnbindableNamesAndEnvSplit(t *testing.T) {
	want := map[string][]policy.RuleID{
		`F=.env; export F; cat "$F"`:                {policy.RuleCommandSecretRead},
		`d=.env; cat "$d"; unset d`:                 {policy.RuleCommandSecretRead},
		`d=.env; cat "$d"; d=$(pwd)`:                {policy.RuleCommandSecretRead},
		`echo '$d'; d=.env; cat $d`:                 {policy.RuleCommandSecretRead},
		`F=.env; export F; echo x > "$F"`:           {policy.RuleSecretPath},
		`<.env env -S cat`:                          {policy.RuleCommandSecretRead},
		`env -S cat <.env`:                          {policy.RuleCommandSecretRead},
		`nice "$@"`:                                 {policy.RuleCommandOperandUnverifiable, policy.RuleCommandSubstitution},
		`env -S cat $X`:                             {policy.RuleCommandOperandUnverifiable},
		`d=lib; d=$zz; echo x > src/$d/../../o.txt`: {policy.RuleOutsideRoot},
	}
	for _, posture := range []string{flags.PostureStrict, flags.PostureDev} {
		g, _ := landGate(t, posture)
		for _, withTask := range []bool{true, false} {
			task := testTask()
			task.AllowedCommands = []string{"*"}
			if !withTask {
				task = nil
			}
			for c, rules := range want {
				d, err := g.EvaluateCommand(c, task)
				if err != nil {
					t.Fatal(err)
				}
				ok := false
				for _, r := range rules {
					ok = ok || d.Rule == r
				}
				if !d.Blocked() || d.Severity != policy.Hard || !ok {
					t.Errorf("%s task=%v %q: got %s %s %q, want a hard %v", posture, withTask, c, d.Action, d.Severity, d.Rule, rules)
				}
			}
		}
	}
}
