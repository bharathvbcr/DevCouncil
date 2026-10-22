package gate

import (
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/flags"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/policy"
)

// A redirection to a stream device is not a write to a file, and refusing it
// as path.outside_root was the gate's commonest false refusal: `ls 2>/dev/null`
// and `cmd >/dev/null 2>&1` are in nearly every shell line an agent writes, and
// each was a hard, ungrantable denial while the same command without the
// redirect ran. The stream is one the command already owns (stdout, stderr, the
// terminal) or a sink that keeps nothing, so there is no file to judge.
func TestARedirectToAStreamDeviceIsNotAWrite(t *testing.T) {
	task := testTask()
	task.AllowedCommands = []string{"cat *"}
	for _, cmd := range []string{
		"cat src/calc.go 2>/dev/null",
		"cat src/calc.go >/dev/null 2>&1",
		"cat src/calc.go &>/dev/null",
		"cat src/calc.go > /dev/null",
		`cat src/calc.go > "/dev/null"`,
		"cat src/calc.go 2>>/dev/null",
		"cat src/calc.go >/dev/stdout",
		"cat src/calc.go 2>/dev/stderr",
		"cat src/calc.go >/dev/fd/2",
		"cat src/calc.go >/dev/tty",
		"cat src/calc.go >/dev/zero",
	} {
		g := newGate(t, nil)
		d, err := g.EvaluateCommand(cmd, task)
		if err != nil {
			t.Fatalf("%q: %v", cmd, err)
		}
		if d.Blocked() {
			t.Errorf("%q refused as %s/%s: %s", cmd, d.Rule, d.Severity, d.Reason)
		}
	}
}

// The other half: only the exact device names are streams. Anything that merely
// starts like one, or climbs out of /dev/fd, is a path and is judged as the
// write it is — outside the root, so refused.
func TestOnlyExactStreamDevicesAreExempt(t *testing.T) {
	task := testTask()
	task.AllowedCommands = []string{"cat *"}
	for _, cmd := range []string{
		"cat src/calc.go > /dev/null.txt",
		"cat src/calc.go > /dev/nullx",
		"cat src/calc.go > /dev/fd/../../etc/passwd",
		"cat src/calc.go > /dev/fd/2x",
		"cat src/calc.go > /dev/disk0",
		"cat src/calc.go > /dev/null/../../etc/hosts",
		"cat src/calc.go > /tmp/out.txt",
	} {
		g := newGate(t, nil)
		d, err := g.EvaluateCommand(cmd, task)
		if err != nil {
			t.Fatalf("%q: %v", cmd, err)
		}
		if !d.Blocked() || d.Rule != policy.RuleOutsideRoot {
			t.Errorf("%q = %s/%s, want deny/%s", cmd, d.Action, d.Rule, policy.RuleOutsideRoot)
		}
	}
}

// A substitution whose inner command was refused carries that refusal's own
// rule and severity. It used to be re-issued as a hard command.substitution,
// which turned a negotiable refusal into an unnegotiable one purely because of
// where the command was written: `date` alone was a soft command.not_allowed a
// grant or a posture could clear, `echo "$(date)"` was a hard denial nothing
// could. The substitution was read to its end, so nothing about it was opaque.
func TestASubstitutionKeepsItsInnerRefusalsSeverity(t *testing.T) {
	task := testTask()
	task.AllowedCommands = []string{"echo *"}

	g := newGate(t, nil)
	alone, err := g.EvaluateCommand("date", task)
	if err != nil {
		t.Fatal(err)
	}
	if !alone.Blocked() || alone.Severity != policy.Soft {
		t.Fatalf("precondition: `date` alone should be a soft refusal, got %s/%s", alone.Rule, alone.Severity)
	}

	for _, cmd := range []string{`echo "$(date)"`, "echo `date`", "echo $(date)"} {
		g := newGate(t, nil)
		d, err := g.EvaluateCommand(cmd, task)
		if err != nil {
			t.Fatal(err)
		}
		if !d.Blocked() {
			t.Fatalf("%q: the inner refusal must still refuse under strict posture: %+v", cmd, d)
		}
		if d.Rule != alone.Rule || d.Severity != alone.Severity {
			t.Errorf("%q = %s/%s, want the inner refusal's own %s/%s",
				cmd, d.Rule, d.Severity, alone.Rule, alone.Severity)
		}
	}

	// Soft is now soft everywhere it appears: the dev posture demotes it, just
	// as it demotes `date` written on its own.
	dev := newGate(t, map[string]string{flags.HarnessPosture: flags.PostureDev})
	if d, err := dev.EvaluateCommand(`echo "$(date)"`, task); err != nil || d.Blocked() {
		t.Errorf("a soft inner refusal must be demotable like the same command alone: %+v %v", d, err)
	}
}

// What must not soften: a hard refusal inside a substitution is still hard, so
// the git-safety rules cannot be stepped around by wrapping them in $(…).
func TestASubstitutionCannotLaunderAHardRefusal(t *testing.T) {
	task := testTask()
	task.AllowedCommands = []string{"echo *", "git *"}
	for _, cmd := range []string{
		`echo "$(git push --force origin main)"`,
		"echo `git commit --no-verify -m x`",
		"echo $(echo $(git push -f))",
	} {
		g := newGate(t, map[string]string{flags.HarnessPosture: flags.PostureDev})
		d, err := g.EvaluateCommand(cmd, task)
		if err != nil {
			t.Fatal(err)
		}
		if !d.Blocked() || d.Severity != policy.Hard {
			t.Errorf("%q = %s/%s/%s, want a hard refusal", cmd, d.Action, d.Rule, d.Severity)
		}
	}
}
