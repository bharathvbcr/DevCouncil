package gate

import (
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/policy"
)

// The no-lease `dev tool` entries match with their redirect tail stripped, as
// every allowlist entry does; the write half is EvaluateRedirects', which
// every host runs. This holds the new entries to that: each redirect target
// reaches the host's write evaluator, with or without a lease, so
// `dev tool call devcouncil_get_diff > .env` is judged as a write to .env and
// never waved through as an allowlisted read.
func TestDevToolRedirectTargetsReachTheWriteEvaluator(t *testing.T) {
	for _, tc := range []struct{ command, target string }{
		{"dev tool call devcouncil_get_diff > .env", ".env"},
		{"dev tool call devcouncil_get_diff >> .devcouncil/state.sqlite", ".devcouncil/state.sqlite"},
		{"dev tool list --json > out.json", "out.json"},
		{"dev tool call devcouncil_next_task 2> err.txt", "err.txt"},
		// The entries that were already there, judged the same way.
		{"git diff > .env", ".env"},
		{"dev map status > out.txt", "out.txt"},
	} {
		for _, taskID := range []string{"", "TASK-1"} {
			var asked []string
			refuse := func(target string) (policy.Decision, error) {
				asked = append(asked, target)
				return policy.Decision{Action: policy.Deny, Rule: policy.RuleUnplannedScope,
					Severity: policy.SeverityOf(policy.RuleUnplannedScope), Reason: "refused", Target: target}, nil
			}
			refusal, err := EvaluateRedirects(tc.command, taskID, true, refuse)
			if err != nil {
				t.Fatalf("EvaluateRedirects(%q): %v", tc.command, err)
			}
			if !refusal.Refused || len(asked) == 0 || asked[0] != tc.target {
				t.Errorf("task %q: %q was not judged as a write to %s (asked %v, refusal %+v)",
					taskID, tc.command, tc.target, asked, refusal)
			}
		}
	}
}
