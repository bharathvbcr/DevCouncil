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
		"git log -1 --format=%s $(pick)",
		"b=.env; git show HEAD:$b",
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
	for _, command := range []string{"git commit --no-gpg-sign -m x", "git commit -m x --no-gpg-sign"} {
		d := gate.EvaluateCommand(command, readOnlyTask())
		if d.Action != Deny || d.Severity != Hard || d.Rule != RuleCommandBypassFlag {
			t.Errorf("%q = %s/%s/%s; want deny/hard/%s", command, d.Action, d.Severity, d.Rule, RuleCommandBypassFlag)
		}
	}
}

// Orientation agents write constantly, read to its end and passed (owner
// decision, 2026-10-24): an unbound variable as a revision or path, a `-c`
// that only changes presentation or skips hooks, a GIT_* that only changes
// paging or points at an index inside the root. A `-c` that runs a program,
// and an option a variable spells, stay refused.
func TestCommonReadOnlyGitOrientationPasses(t *testing.T) {
	gate := CommandGate{HardRules: true, Root: readOnlyRoot(t)}
	for _, command := range []string{
		"git log --oneline main..$b",
		"git log --oneline -1 $b",
		`git log -1 --format=%s "$b"`,
		"git show $c:README.md",
		"git -c core.pager=cat log --oneline -1",
		"git -c core.quotepath=off status --short",
		`git -c "core.pager=less -R" diff`,
		"git -c color.ui=always diff --stat",
		"git -c core.hooksPath=/dev/null log -1 --oneline",
		"GIT_PAGER=cat git log -3",
		"GIT_INDEX_FILE=build/rank.index git diff --cached --stat HEAD",
		"for b in main dev; do git log --oneline -1 $b; done",
	} {
		if d := gate.EvaluateCommand(command, readOnlyTask()); d.Rule == RuleCommandGitReadArgument {
			t.Errorf("%q refused as %s: %s", command, d.Rule, d.Reason)
		}
	}
	for _, command := range []string{
		"git -c core.pager=rm diff",
		"git -c diff.external=x diff",
		"git -c core.sshCommand=x log",
		"git -c alias.log=!sh log",
		"git -c $K=v log",
		"GIT_EXTERNAL_DIFF=x git diff",
		"GIT_INDEX_FILE=../elsewhere git diff --cached",
		"GIT_PAGER=sh git log",
		"git diff --$X",
	} {
		if d := gate.EvaluateCommand(command, readOnlyTask()); d.Rule != RuleCommandGitReadArgument {
			t.Errorf("%q = %s/%s (%s); want %s", command, d.Action, d.Rule, d.Reason, RuleCommandGitReadArgument)
		}
	}
}

// Skipping the repository's own hooks is the person's call (owner decision,
// 2026-10-24): --no-verify on a commit or a push is no longer a git-safety
// refusal, while the force push beside it still is.
func TestNoVerifyIsNotAGitSafetyRefusal(t *testing.T) {
	for _, command := range []string{
		"git commit --no-verify -m x",
		"git push --no-verify origin feature",
		`git commit "--no-verify" -m x`,
	} {
		if d := GitSafety(command); d.Action == Deny {
			t.Errorf("GitSafety(%q) = deny %s: %s", command, d.Rule, d.Reason)
		}
	}
	if d := GitSafety("git push --no-verify --force origin main"); d.Action != Deny || d.Rule != RuleCommandForcePush {
		t.Errorf("a force push with --no-verify must still refuse as a force push, got %s/%s", d.Action, d.Rule)
	}
}

// A substituted read-only git command is judged by the same rung, and
// judgeSubstitutions returns the inner refusal under its own rule.
func TestSubstitutedReadOnlyGitArgumentReportsTheReadRule(t *testing.T) {
	gate := CommandGate{HardRules: true, Root: readOnlyRoot(t)}
	for _, command := range []string{"$(git log --output=x)", `echo "$(git log -1 --format=%s --output=x)"`} {
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
