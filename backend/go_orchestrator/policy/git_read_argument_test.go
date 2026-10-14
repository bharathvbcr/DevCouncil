package policy

import (
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
)

// The read-only argument rung used to report every refusal as
// command.bypass_flag, a rule that names the verification-bypass flags
// (--no-verify, --no-gpg-sign). `git log -1 --format=%s "$b"` was refused as
// [command.bypass_flag], which told the user they had tried to skip a hook
// when they had only written an argument the gate cannot read. The rung has
// its own rule now; these tests pin the label as well as the refusal.

func TestReadOnlyGitArgumentRefusalsCarryTheirOwnRule(t *testing.T) {
	gate := CommandGate{HardRules: true, Root: readOnlyRoot(t)}
	for _, command := range []string{
		"git log -1 --format=%s $b",
		`git log -1 --format=%s "$b"`,
		"git diff --output=x",
		"git diff --no-index a b",
		"git show HEAD:.env",
		"git -c core.pager=x diff",
		"git diff ../outside",
	} {
		for name, task := range map[string]*dc.Task{"no lease": nil, "lease": readOnlyTask()} {
			d := gate.EvaluateCommand(command, task)
			if d.Action != Deny || d.Severity != Hard || d.Rule != RuleCommandGitReadArgument {
				t.Errorf("%s: %q = %s/%s/%s (%s); want deny/hard/%s",
					name, command, d.Action, d.Severity, d.Rule, d.Reason, RuleCommandGitReadArgument)
			}
		}
	}
}

func TestVerificationBypassFlagsKeepTheirRule(t *testing.T) {
	gate := CommandGate{HardRules: true, Root: readOnlyRoot(t)}
	for _, command := range []string{"git commit --no-verify -m x", "git commit --no-gpg-sign -m x"} {
		d := gate.EvaluateCommand(command, readOnlyTask())
		if d.Action != Deny || d.Severity != Hard || d.Rule != RuleCommandBypassFlag {
			t.Errorf("%q = %s/%s/%s; want deny/hard/%s", command, d.Action, d.Severity, d.Rule, RuleCommandBypassFlag)
		}
	}
}

// A substituted read-only git command is judged by the same rung, and
// judgeSubstitutions returns the inner refusal under its own rule.
func TestSubstitutedReadOnlyGitArgumentReportsTheReadRule(t *testing.T) {
	gate := CommandGate{HardRules: true, Root: readOnlyRoot(t)}
	for _, command := range []string{"$(git log $b)", `echo "$(git log -1 --format=%s $b)"`} {
		d := gate.EvaluateCommand(command, readOnlyTask())
		if d.Action != Deny || d.Severity != Hard || d.Rule != RuleCommandGitReadArgument {
			t.Errorf("%q = %s/%s/%s (%s); want deny/hard/%s",
				command, d.Action, d.Severity, d.Rule, d.Reason, RuleCommandGitReadArgument)
		}
	}
}

func TestGitReadArgumentIsHardButNotUnreadable(t *testing.T) {
	if SeverityOf(RuleCommandGitReadArgument) != Hard {
		t.Errorf("%s severity = %s, want hard", RuleCommandGitReadArgument, SeverityOf(RuleCommandGitReadArgument))
	}
	if !IsCommandRule(RuleCommandGitReadArgument) {
		t.Errorf("%s is not classified as a command rule", RuleCommandGitReadArgument)
	}
	// A host that softens unreadable refusals (policy.unreadable_only) must not
	// be able to soften `git diff --output=x`: the rung read the argument to
	// its end and refused what it says.
	if IsUnreadableRule(RuleCommandGitReadArgument) {
		t.Errorf("%s must not be in the unreadable class", RuleCommandGitReadArgument)
	}
}
