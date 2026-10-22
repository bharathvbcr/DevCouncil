package gate

import (
	"slices"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/policy"
)

// A refusal for an unreadable construct is the one hard refusal whose command
// a host may still run — it means "I could not place this", not "this is
// forbidden" — so its redirection targets must be judged, and a refused target
// replaces it. Before, a hard command decision stopped here and `cd src && echo
// x > .env` was refused only as a directory change; a host softening that would
// have written .env having never shown it to the write gate.
func TestAnUnreadableRefusalStillHasItsRedirectsJudged(t *testing.T) {
	task := testTask()
	task.AllowedCommands = []string{"echo *", "ls *"}
	g := newGate(t, nil)
	d, err := g.EvaluateCommand("cd src && echo x > .env", task)
	if err != nil {
		t.Fatal(err)
	}
	if !d.Blocked() || d.Rule != policy.RuleSecretPath {
		t.Fatalf("got %s/%s, want the secret write refused: %s", d.Action, d.Rule, d.Reason)
	}
	if slices.Contains(d.Degraded, policy.DegradedUnreadableOnly) {
		t.Fatalf("a secret write must not carry the unreadable-only marker: %+v", d)
	}
}

// The marker says exactly one thing: every refusal in the line was an
// unreadable construct, and the redirects were judged and passed. Nothing else
// may carry it.
func TestOnlyAnUnreadableOnlyLineCarriesTheMarker(t *testing.T) {
	task := testTask()
	task.AllowedCommands = []string{"echo *", "ls *", "git *", "cat *"}
	marked := func(cmd string) (policy.Decision, bool) {
		t.Helper()
		g := newGate(t, nil)
		d, err := g.EvaluateCommand(cmd, task)
		if err != nil {
			t.Fatal(err)
		}
		return d, slices.Contains(d.Degraded, policy.DegradedUnreadableOnly)
	}
	for _, cmd := range []string{"cd src && ls", "cat <<'EOF' > src/calc.go", "(cd src && ls)"} {
		d, ok := marked(cmd)
		if !ok || !d.Blocked() || d.Severity != policy.Hard {
			t.Errorf("%q: want a hard unreadable refusal carrying the marker, got %+v", cmd, d)
		}
	}
	for _, cmd := range []string{
		"git push --force origin main",
		"cd src && git push --force origin main",
		"cd src && echo x > .env",
		"cat src/calc.go > /tmp/out.txt",
		"ls src",
	} {
		if d, ok := marked(cmd); ok {
			t.Errorf("%q must not carry the marker: %+v", cmd, d)
		}
	}
}
