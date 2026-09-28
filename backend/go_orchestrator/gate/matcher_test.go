package gate

import (
	"errors"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/flags"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/policy"
)

type downMatcher struct{ calls *int }

func (m downMatcher) MatchAny([]string, string) (bool, error) {
	*m.calls++
	return false, errors.New("engine is down")
}

func (m downMatcher) MatchAnyFold([]string, string) (bool, error) {
	*m.calls++
	return false, errors.New("engine is down")
}

// Gate.Matcher reaches every ladder the gate composes — write, read, command
// and a command's redirect target — and an engine that cannot answer is a
// hard denial that neither the dev posture nor the mode flag demotes.
func TestGateMatcherReachesEveryLadderAndFailsClosed(t *testing.T) {
	for _, posture := range []string{flags.PostureStrict, flags.PostureDev} {
		calls := 0
		g := newGate(t, map[string]string{flags.HarnessPosture: posture})
		g.Matcher = downMatcher{calls: &calls}
		task := testTask()
		task.AllowedCommands = []string{"echo *", "go test *"}

		check := func(what string, d policy.Decision, err error, rule policy.RuleID) {
			t.Helper()
			if err != nil {
				t.Fatalf("%s %s: %v", posture, what, err)
			}
			if d.Action != policy.Deny || d.Rule != rule || d.Severity != policy.Hard {
				t.Errorf("%s %s: %s/%s/%s, want deny/%s/hard (%s)", posture, what, d.Action, d.Rule, d.Severity, rule, d.Reason)
			}
		}
		d, err := g.EvaluateWrite("src/calc.go", task, dc.OpWrite)
		check("write", d, err, policy.RulePathEngineUnavailable)
		d, err = g.EvaluateRead("src/calc.go", task)
		check("read", d, err, policy.RulePathEngineUnavailable)
		d, err = g.EvaluateCommand("go test ./...", task)
		check("command", d, err, policy.RuleCommandEngineUnavailable)
		if calls == 0 {
			t.Fatalf("%s: the matcher was never asked", posture)
		}
	}
}
