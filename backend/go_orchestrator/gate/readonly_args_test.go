package gate

import (
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/policy"
)

// The audit's three reproductions, through the real gate under strict: no
// lease for the git forms, and a lease that lists pytest for the test runner.
// Each was an allow; each is now a hard refusal that no grant or mode demotes.
func TestReadOnlyAndTestRunnerArgumentsCannotWriteOrReadOutsideTheRules(t *testing.T) {
	for _, tc := range []struct {
		command string
		hasTask bool
	}{
		{"git diff --output=.git/config", false},
		{"git diff --output=../x", false},
		{"git diff --outp=../x", false},
		{"git diff --no-index /dev/null .env", false},
		{"git diff --no-index /dev/null /etc/passwd", false},
		{"git diff -- .env", false},
		{"pytest --junitxml=.env x", true},
		{"pytest --basetemp=.git x", true},
		{"python -m pytest --junitxml=../outside.xml x", true},
	} {
		g := newGate(t, nil)
		task := testTask()
		if !tc.hasTask {
			task = nil
		}
		d, err := g.EvaluateCommand(tc.command, task)
		if err != nil {
			t.Fatalf("%q: %v", tc.command, err)
		}
		if !d.Blocked() || d.Severity != policy.Hard {
			t.Errorf("%q = %s/%s/%s; want a hard refusal", tc.command, d.Action, d.Severity, d.Rule)
		}
	}
}

func TestPlainOrientationStillRunsWithNoLease(t *testing.T) {
	for _, command := range []string{"git status", "git diff", "git diff --stat", "git diff HEAD~1 -- src/calc.go"} {
		g := newGate(t, nil)
		d, err := g.EvaluateCommand(command, nil)
		if err != nil {
			t.Fatal(err)
		}
		if d.Blocked() {
			t.Errorf("%q refused as %s: %s", command, d.Rule, d.Reason)
		}
	}
}
