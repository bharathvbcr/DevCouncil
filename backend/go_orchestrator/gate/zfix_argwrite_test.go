package gate

import (
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/flags"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/policy"
)

// EvaluateRedirects is the one write rung every host calls, so a file a
// command writes through its arguments has to reach the caller's evaluator
// exactly as a redirection target does. Before, `sed -i s/a/b/ docs/other.md`
// put nothing through the evaluator at all, and a host whose posture demotes
// command.not_allowed allowed it while refusing `echo x > docs/other.md`.
func TestEvaluateRedirectsJudgesArgumentWrites(t *testing.T) {
	refuseOther := func(asked *[]string) WriteEvaluator {
		return func(target string) (policy.Decision, error) {
			*asked = append(*asked, target)
			if target == "docs/other.md" {
				return policy.Decision{Action: policy.Deny, Rule: policy.RuleUnplannedScope,
					Severity: policy.SeverityOf(policy.RuleUnplannedScope), Reason: "not planned", Target: target}, nil
			}
			return policy.Decision{Action: policy.Allow, Target: target}, nil
		}
	}
	for _, command := range []string{
		"sed -i s/a/b/ docs/other.md",
		"sed -i '' s/a/b/ docs/other.md",
		"tee docs/other.md",
		"cp a docs/other.md",
		"mv a docs/other.md",
		"cp src/other.md docs/",
		"true && echo $(tee -a docs/other.md)",
	} {
		var asked []string
		refusal, err := EvaluateRedirects(command, "TASK-1", refuseOther(&asked))
		if err != nil {
			t.Fatalf("EvaluateRedirects(%q): %v", command, err)
		}
		if !refusal.Refused || !refusal.FromTarget || refusal.Decision.Rule != policy.RuleUnplannedScope {
			t.Errorf("EvaluateRedirects(%q) = %+v (asked %v); want the evaluator's scope.unplanned refusal",
				command, refusal, asked)
		}
	}

	// The same commands, with an evaluator that allows the path, are allowed:
	// the rung adds judgement, not a refusal of its own.
	allowAll := func(target string) (policy.Decision, error) {
		return policy.Decision{Action: policy.Allow, Target: target}, nil
	}
	for _, command := range []string{
		"sed -i s/a/b/ docs/other.md",
		"tee docs/other.md",
		"cp a docs/other.md",
		"mv a docs/other.md",
		"sed -i s/a/b/ src/*.go",
	} {
		refusal, err := EvaluateRedirects(command, "TASK-1", allowAll)
		if err != nil {
			t.Fatalf("EvaluateRedirects(%q): %v", command, err)
		}
		if refusal.Refused {
			t.Errorf("EvaluateRedirects(%q) refused (%+v) though the evaluator allows every path", command, refusal)
		}
	}

	// An operand only the shell can resolve is refused the way `> $VAR` is,
	// without the evaluator being asked to judge a path nobody can name.
	var asked []string
	refusal, err := EvaluateRedirects(`sed -i s/a/b/ "$F"`, "TASK-1", refuseOther(&asked))
	if err != nil {
		t.Fatal(err)
	}
	if !refusal.Refused || refusal.FromTarget || refusal.Decision.Rule != policy.RuleCommandSubstitution {
		t.Errorf("expansion operand: got %+v, want a synthesised command.substitution refusal", refusal)
	}
}

// Through the real gate: a posture that demotes the soft command refusal must
// still refuse the unplanned write, and a planned destination stays allowed.
func TestDemotedCommandStillHasItsArgumentWritesJudged(t *testing.T) {
	task := &dc.Task{
		ID:           "TASK-001",
		PlannedFiles: []dc.PlannedFile{{Path: "src/calc.go", AllowedChange: dc.ChangeModify}},
	}
	for _, command := range []string{
		"sed -i s/a/b/ .env",
		"echo x | tee .env",
		"cp src/calc.go .env",
		"mv src/calc.go .env",
	} {
		g := newGate(t, map[string]string{flags.HarnessPosture: flags.PostureDev})
		d, err := g.EvaluateCommand(command, task)
		if err != nil {
			t.Fatalf("EvaluateCommand(%q): %v", command, err)
		}
		if !d.Blocked() || d.Rule != policy.RuleSecretPath {
			t.Errorf("dev posture: %q = %s/%s (%s); want the .env write refused as %s",
				command, d.Action, d.Rule, d.Reason, policy.RuleSecretPath)
		}
	}
	g := newGate(t, map[string]string{flags.HarnessPosture: flags.PostureDev})
	d, err := g.EvaluateCommand("sed -i s/a/b/ src/calc.go", task)
	if err != nil {
		t.Fatal(err)
	}
	if d.Blocked() {
		t.Errorf("dev posture: in-place edit of the planned file was refused: %s/%s (%s)", d.Action, d.Rule, d.Reason)
	}
}
