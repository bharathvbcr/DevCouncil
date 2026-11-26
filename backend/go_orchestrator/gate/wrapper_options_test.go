package gate

import (
	"slices"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/flags"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/policy"
)

var wrapperTaskCommands = []string{"env *", "sudo *", "nice *", "timeout *", "stdbuf *", "ionice *", "time *", "/usr/bin/time *", "cat *",
	"git show *", "rm *", "cp *", "sed *", "tee *", "go test *", "go build *", "make *", "ls *", "devcouncil *", "xargs *", "find *"}

type wrapperRow struct {
	line  string
	rules []policy.RuleID
}

func rules(r ...policy.RuleID) []policy.RuleID { return r }

// ft-46efdb70: a wrapper followed by an option still runs the wrapped command,
// and every operand rung judges it. Each row names the rule that refuses it;
// a refusal for what the gate could not read (command.operand.unverifiable)
// is softenable by a host, so a row that names a credential or a protected
// path must not fall to it.
func TestWrapperOptionsReachOperandRungs(t *testing.T) {
	secret := rules(policy.RuleCommandSecretRead, policy.RuleSecretPath)
	protected := rules(policy.RuleRestrictedPath)
	refused := []wrapperRow{
		{"env -i cat .env", secret}, {"env -u HOME cat .env", secret}, {"env FOO=1 cat .env", secret},
		{"sudo -u root cat .env", secret}, {"sudo -E cat .env", secret}, {"nice -n 5 cat .env", secret},
		{"timeout 5 cat .env", secret}, {"timeout -s KILL 5 cat .env", secret}, {"stdbuf -o0 cat .env", secret},
		{"ionice -c3 cat .env", secret},
		{"env -i git show HEAD:.env", rules(policy.RuleCommandGitReadArgument, policy.RuleCommandSecretRead)},
		{"env -i rm -rf .git", protected}, {"sudo -u root cp x .git/hooks/pre-commit", protected},
		{"env -i sed -i s/a/b/ .env", secret}, {"nice -n 5 tee .git/config", protected},
		{"env -i devcouncil review record", rules(policy.RuleCommandHarnessOnly)},
		{"sudo -u root devcouncil review record", rules(policy.RuleCommandHarnessOnly)},
		{"timeout -s KILL 5 devcouncil review record", rules(policy.RuleCommandHarnessOnly)},
		{"env -i dcstore review-append", rules(policy.RuleCommandHarnessOnly)},
		// R1: an option that moves the working directory or root is refused like cd.
		{"env -C .git rm -rf hooks", rules(policy.RuleCommandDirectoryChange)},
		{"env --chdir=.git rm -rf hooks", rules(policy.RuleCommandDirectoryChange)},
		{"sudo -D .git rm -rf hooks", rules(policy.RuleCommandDirectoryChange)},
		{"env -C / rm -rf tmp/zz", rules(policy.RuleCommandDirectoryChange)},
		{"sudo -R / ls", rules(policy.RuleCommandDirectoryChange)},
		// R2: an option that makes the wrapper write a file names a write target.
		{"/usr/bin/time -o .env true", secret}, {"time -o .git/config true", protected},
		{"time --output=.git/hooks/pre-commit true", protected}, {"env -i time -o .env true", secret},
		{"time -a -o .git/config true", protected},
		// R3: an unknown wrapper option does not demote a spelled credential read.
		{"env --frobnicate cat .env", secret}, {"env -a x cat .env", secret}, {"sudo --frobnicate cat .env", secret},
		// R4: nothing is skipped behind an option the table does not know.
		{"xargs env --frobnicate rm -rf .git", protected}, {"find . -exec env --frobnicate rm -rf .git \\;", protected},
		{"env --frobnicate rm -rf .git", protected}, {"env -S \"cat .env\"", secret}, {"env -S \"rm -rf .git\"", protected},
		{"sudo -e .env", secret}, {"sudo --edit .git/config", protected},
		// an unknown option with nothing spelled behind it still refuses, as unverifiable.
		{"env --frobnicate cat README.md", rules(policy.RuleCommandOperandUnverifiable)},
		{"sudo --frobnicate ls", rules(policy.RuleCommandOperandUnverifiable)},
	}
	allowed := []string{
		"env -i go test ./...", "timeout 60 go build ./...", "nice -n 10 make", "env FOO=1 cat README.md", "sudo -u root ls",
	}
	for _, posture := range []string{flags.PostureStrict, flags.PostureDev} {
		g, _ := landGate(t, posture)
		for _, withTask := range []bool{true, false} {
			var task *dc.Task
			if withTask {
				task = testTask()
				task.AllowedCommands = wrapperTaskCommands
			}
			for _, row := range refused {
				d, err := g.EvaluateCommand(row.line, task)
				if err != nil {
					t.Fatal(err)
				}
				t.Logf("ROW %s task=%v %q -> %s %s %q %v", posture, withTask, row.line, d.Action, d.Severity, d.Rule, d.Degraded)
				want := row.rules
				if posture == flags.PostureStrict && slices.Contains(want, policy.RuleCommandDirectoryChange) {
					// As for a plain cd, strict reports the scope refusal first.
					want = append(slices.Clone(want), policy.RuleUnplannedScope, policy.RuleNoTask)
				}
				if !d.Blocked() || !slices.Contains(want, d.Rule) {
					t.Errorf("%s task=%v %q: got %s %q, want one of %v", posture, withTask, row.line, d.Action, d.Rule, want)
					continue
				}
				if d.Rule != policy.RuleCommandOperandUnverifiable && !slices.Contains(row.rules, policy.RuleCommandDirectoryChange) {
					if d.Severity != policy.Hard || slices.Contains(d.Degraded, policy.DegradedUnreadableOnly) {
						t.Errorf("%s task=%v %q: %q is softenable (severity %v, degraded %v), want a hard refusal", posture, withTask, row.line, d.Rule, d.Severity, d.Degraded)
					}
				}
			}
			if !withTask {
				continue
			}
			for _, c := range allowed {
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
}
