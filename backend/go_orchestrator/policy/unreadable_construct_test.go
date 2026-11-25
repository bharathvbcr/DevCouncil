package policy

import (
	"strings"
	"testing"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
)

// The ladder returned the first refused clause, so a refusal about the gate's
// own reach (a `cd`, a heredoc) hid a refusal about what the line does. Measured
// against `manvi serve`: `cd src && git push --force origin main` came back
// command.directory_change, and the force push was never reported. A host that
// treats "I could not read this" more gently than "this is forbidden" — which
// is the only honest way to treat it — would have run the push. A refusal for
// what a line does now always wins over one for what the gate could not read.
func TestABehaviouralRefusalIsNeverHiddenBehindAnUnreadableOne(t *testing.T) {
	task := &dc.Task{ID: "TASK-001", AllowedCommands: []string{"echo *", "git *", "cat *", "ls *"}}
	gate := CommandGate{HardRules: true}
	for cmd, want := range map[string]RuleID{
		"cd src && git push --force origin main":        RuleCommandForcePush,
		"cd src || git commit --no-gpg-sign -m x":       RuleCommandBypassFlag,
		"pushd src; git push -f":                        RuleCommandForcePush,
		"git push --force origin main <<'EOF'":          RuleCommandForcePush,
		`git commit --no-gpg-sign -m "$(cat <<'EOF'`:    RuleCommandBypassFlag,
		`echo "$(cd src && git push -f)"`:               RuleCommandForcePush,
		"cat <<'EOF'\ngit push --force\nEOF":            RuleCommandForcePush,
		`echo "$(cat <<'EOF'` + "\ngit push -f\nEOF\n)": RuleCommandForcePush,
		"git -C src push --force":                       RuleCommandForcePush,
		// A soft refusal in front is the same masking, and worse: soft is
		// demotable, so `date` hid the push from every posture that demotes.
		"date; git push --force origin main":    RuleCommandForcePush,
		"nope && git commit --no-gpg-sign -m x": RuleCommandBypassFlag,
	} {
		d := gate.EvaluateCommand(cmd, task)
		if d.Action != Deny || d.Rule != want {
			t.Errorf("%q = %s/%s, want deny/%s", cmd, d.Action, d.Rule, want)
		}
	}
}

// The other order matters too: a soft refusal must not hide an unreadable
// construct either. `date; cd .. && echo x > f` used to return `date`'s soft
// command.not_allowed, which a posture demotes — so the `cd` was never refused
// at all and the redirect was judged against the wrong directory.
func TestAnUnreadableConstructIsNotHiddenBehindASoftRefusal(t *testing.T) {
	task := &dc.Task{ID: "TASK-001", AllowedCommands: []string{"echo *"}}
	gate := CommandGate{HardRules: true}
	for _, cmd := range []string{
		"date; cd .. && echo x > f",
		"date && cd src",
		"cd src; date",
	} {
		d := gate.EvaluateCommand(cmd, task)
		if d.Action != Deny || d.Rule != RuleCommandDirectoryChange {
			t.Errorf("%q = %s/%s, want deny/%s", cmd, d.Action, d.Rule, RuleCommandDirectoryChange)
		}
	}
}

// A `cd` inside a subshell or group still moves where the rest of that group's
// relative paths land; it was missed because the rung only looked at the start
// of a clause.
func TestADirectoryChangeInsideAGroupIsStillADirectoryChange(t *testing.T) {
	task := &dc.Task{ID: "TASK-001", AllowedCommands: []string{"echo *", "ls *"}}
	gate := CommandGate{HardRules: true}
	for _, cmd := range []string{
		"(cd src && ls)",
		"( cd ..; echo x > allowed.txt )",
		"{ cd src; ls; }",
		"builtin cd src",
		"command cd ..",
	} {
		d := gate.EvaluateCommand(cmd, task)
		if d.Action != Deny || d.Rule != RuleCommandDirectoryChange {
			t.Errorf("%q = %s/%s, want deny/%s", cmd, d.Action, d.Rule, RuleCommandDirectoryChange)
		}
	}
	// Words that merely start like it are not it.
	for _, cmd := range []string{"echo cdrom", "ls cd", "echo (cd)"} {
		if d := gate.EvaluateCommand(cmd, task); d.Rule == RuleCommandDirectoryChange {
			t.Errorf("%q misread as a directory change", cmd)
		}
	}
}

// Judging every clause instead of stopping at the first refusal must not turn
// the length cap into a suggestion. The worst case is a line that is nothing
// but unreadable clauses, each of which used to end the walk at once.
//
// The bound is on how the cost grows: a quarter-length line and a full-length
// one are timed in turn, best of three each, and the full one may cost at most
// 8x the quarter. Linear work gives about 4x on any host (3.9-4.3x measured,
// under -race and a parallel suite too). A quadratic term fails the check once
// it costs more than twice the linear work at the cap; one that small is not
// caught. A tight wall-clock limit could not tell growth from a busy runner: a
// loaded macOS CI host took 5.03 s for a line that takes 0.27 s on an idle
// one. The 60 s backstop is only for a hang or a gross slowdown, which a ratio
// alone would pass.
func TestScanningEveryClauseStaysBounded(t *testing.T) {
	gate := CommandGate{HardRules: true}
	task := &dc.Task{ID: "TASK-001"}
	clauses := (maxCommandBytes - 16) / 8
	full := strings.Repeat("cd x && ", clauses) + "ls"
	quarter := strings.Repeat("cd x && ", clauses/4) + "ls"
	timed := func(line string) (time.Duration, Decision) {
		start := time.Now()
		d := gate.EvaluateCommand(line, task)
		return time.Since(start), d
	}
	bestFull, bestQuarter := time.Duration(1<<62), time.Duration(1<<62)
	for range 3 {
		q, _ := timed(quarter)
		f, d := timed(full)
		if d.Rule != RuleCommandDirectoryChange {
			t.Fatalf("got %s", d.Rule)
		}
		bestQuarter, bestFull = min(bestQuarter, q), min(bestFull, f)
	}
	if bestFull > 60*time.Second {
		t.Fatalf("a %d-byte line of directory changes took %s at best", len(full), bestFull)
	}
	ratio := float64(bestFull) / float64(max(bestQuarter, time.Microsecond))
	t.Logf("full %s, quarter %s, %.1fx", bestFull, bestQuarter, ratio)
	if ratio > 8 {
		t.Fatalf("a %d-byte line took %s and a %d-byte one %s: %.1fx for 4x the input",
			len(full), bestFull, len(quarter), bestQuarter, ratio)
	}
}

func TestOnlyTheReachRulesAreUnreadable(t *testing.T) {
	for _, r := range []RuleID{RuleCommandDirectoryChange, RuleCommandHeredoc,
		RuleCommandSubstitution, RuleCommandReparse} {
		if !IsUnreadableRule(r) {
			t.Errorf("%s should be unreadable", r)
		}
	}
	for _, r := range []RuleID{RuleCommandForcePush, RuleCommandBypassFlag, RuleCommandGitReadArgument, RuleOutsideRoot,
		RuleSecretPath, RuleCommandNotAllowed, RuleCommandTooLong, RuleCommandEngineUnavailable} {
		if IsUnreadableRule(r) {
			t.Errorf("%s must not be unreadable", r)
		}
	}
}
