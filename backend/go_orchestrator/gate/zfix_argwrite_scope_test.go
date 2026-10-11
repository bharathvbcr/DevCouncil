package gate

import (
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/flags"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/policy"
)

// A command that writes a file is judged as that write. Before, only sed -i,
// tee, cp and mv were read, so with a task scope bound `rm`, `truncate`,
// `touch`, `dd of=`, `perl -i`, `install`, `ln`, `rsync` and `patch FILE`
// changed a path the file gate refuses, and the command gate allowed it.
//
// The table is every such command against one target in each class the file
// gate distinguishes. Each refusal must be the file gate's own rule — the
// command rung owns no scope rule of its own.
func scopedWriteTask() *dc.Task {
	allowed := []string{
		"rm *", "truncate *", "touch *", "dd *", "perl *", "install *", "ln *",
		"rsync *", "patch *", "sed *", "xargs *", "find *", "git apply *",
	}
	return &dc.Task{
		ID:              "TASK-001",
		PlannedFiles:    []dc.PlannedFile{{Path: "src/calc.go", AllowedChange: dc.ChangeModify}},
		AllowedCommands: allowed,
	}
}

var scopedWriteCommands = map[string]string{
	"rm":          "rm -f TARGET",
	"truncate":    "truncate -s 0 TARGET",
	"touch":       "touch TARGET",
	"dd":          "dd if=/dev/zero of=TARGET bs=1 count=1",
	"perl -i":     "perl -pi -e 's/a/b/' TARGET",
	"perl -i.bak": "perl -i.bak -pe 's/a/b/' TARGET",
	"install":     "install seed.txt TARGET",
	"ln":          "ln -sf seed.txt TARGET",
	"rsync":       "rsync seed.txt TARGET",
	"patch":       "patch TARGET fix.diff",
	"patch <":     "patch TARGET < fix.diff",
	"sed -i":      "sed -i s/a/b/ TARGET",
}

func TestCommandGateJudgesEveryArgumentWriteAsTheFileGateDoes(t *testing.T) {
	classes := []struct {
		name, target string
		blocked      bool
		rule         policy.RuleID
	}{
		{"planned", "src/calc.go", false, ""},
		{"unplanned", "internal/other.go", true, policy.RuleUnplannedScope},
		{"secret", ".env", true, policy.RuleSecretPath},
		{"outside root", "../outside.txt", true, policy.RuleOutsideRoot},
	}
	for name, template := range scopedWriteCommands {
		for _, class := range classes {
			// A removal is judged as a delete: the planned file is planned for
			// modification, so removing it is outside what the task may do.
			if name == "rm" && class.name == "planned" {
				class.blocked, class.rule = true, policy.RuleOperation
			}
			command := strings.ReplaceAll(template, "TARGET", class.target)
			t.Run(name+"/"+class.name, func(t *testing.T) {
				g := newGate(t, nil)
				d, err := g.EvaluateCommand(command, scopedWriteTask())
				if err != nil {
					t.Fatalf("EvaluateCommand(%q): %v", command, err)
				}
				if d.Blocked() != class.blocked {
					t.Fatalf("%q: blocked=%v (%s/%s: %s), want blocked=%v",
						command, d.Blocked(), d.Action, d.Rule, d.Reason, class.blocked)
				}
				if class.blocked {
					if d.Rule != class.rule {
						t.Errorf("%q: rule %s, want the file gate's %s", command, d.Rule, class.rule)
					}
					want := policy.SeverityOf(class.rule)
					if d.Severity != want {
						t.Errorf("%q: severity %s, want the file gate's %s", command, d.Severity, want)
					}
				}
			})
		}
	}
}

// The verdict must be the one policy.check.file gives the same path for the
// operation the command performs: rm removes it.
func TestCommandGateVerdictEqualsFileGateVerdict(t *testing.T) {
	for _, target := range []string{"src/calc.go", "internal/other.go", ".env", "../outside.txt"} {
		g := newGate(t, nil)
		fileVerdict, err := g.EvaluateWrite(target, scopedWriteTask(), dc.OpDelete)
		if err != nil {
			t.Fatal(err)
		}
		g2 := newGate(t, nil)
		cmdVerdict, err := g2.EvaluateCommand("rm -f "+target, scopedWriteTask())
		if err != nil {
			t.Fatal(err)
		}
		if fileVerdict.Blocked() != cmdVerdict.Blocked() || fileVerdict.Rule != cmdVerdict.Rule {
			t.Errorf("%s: file gate %s/%s, command gate %s/%s",
				target, fileVerdict.Action, fileVerdict.Rule, cmdVerdict.Action, cmdVerdict.Rule)
		}
	}
}

// `rm -rf src` removes every file under src, not only the planned one; the
// directory is not itself planned, so it is judged as the unplanned path it is.
func TestRecursiveRemoveOfADirectoryHoldingAPlannedFileIsRefused(t *testing.T) {
	for _, command := range []string{"rm -rf src", "rm -rf src/", "rm -r ./src", "rm -rf *"} {
		g := newGate(t, nil)
		d, err := g.EvaluateCommand(command, scopedWriteTask())
		if err != nil {
			t.Fatal(err)
		}
		if !d.Blocked() || d.Rule != policy.RuleUnplannedScope {
			t.Errorf("%q = %s/%s, want blocked as %s", command, d.Action, d.Rule, policy.RuleUnplannedScope)
		}
	}
}

// Writes whose targets only the program's input or a patch body names cannot be
// judged, so under a bound scope they are refused with a reason.
func TestUnjudgeableWritesAreRefusedUnderABoundScope(t *testing.T) {
	for _, command := range []string{
		"git apply fix.diff",
		"git apply --index fix.diff",
		"git -C . apply fix.diff",
		"patch -p1 < fix.diff",
		"patch -p1 -i fix.diff",
		"xargs rm",
		"xargs -0 rm -f",
		"xargs -I{} cp {} out/",
		"xargs -n1 sed -i s/a/b/",
		"rm -rf src/*",
		"find . -name '*.tmp' -delete",
		"find . -name '*.tmp' -exec rm {} \\;",
		"find . -exec touch {} +",
		"find src -type f -execdir truncate -s 0 {} ';'",
		"echo $(find . -delete)",
	} {
		g := newGate(t, nil)
		d, err := g.EvaluateCommand(command, scopedWriteTask())
		if err != nil {
			t.Fatal(err)
		}
		if !d.Blocked() || d.Rule != policy.RuleCommandSubstitution || !strings.Contains(d.Reason, "scope") {
			t.Errorf("%q = %s/%s (%s), want a scope-naming %s refusal",
				command, d.Action, d.Rule, d.Reason, policy.RuleCommandSubstitution)
		}
	}
	// Read-only forms of the same commands write nothing.
	for _, command := range []string{
		"git apply --check fix.diff",
		"git apply --stat fix.diff",
		"patch --dry-run -p1 < fix.diff",
		"find . -name '*.go' -exec grep -l x {} +",
		"find . -type f -print",
		"xargs grep -l x",
		"xargs -n1 echo",
	} {
		g := newGate(t, nil)
		d, err := g.EvaluateCommand(command, scopedWriteTask())
		if err != nil {
			t.Fatal(err)
		}
		if d.Blocked() && d.Rule == policy.RuleCommandSubstitution {
			t.Errorf("%q refused as %s (%s); it writes nothing", command, d.Rule, d.Reason)
		}
	}
}

// With no scope bound, the unjudgeable forms are not refused by this rung, and
// a literal in-root non-secret target is still allowed.
func TestUnscopedHostBehaviourIsUnchanged(t *testing.T) {
	allowAll := func(target string) (policy.Decision, error) {
		return policy.Decision{Action: policy.Allow, Target: target}, nil
	}
	for _, command := range []string{
		"rm -f build/out.o", "touch x", "xargs rm", "find . -delete", "git apply fix.diff",
		"patch -p1 < fix.diff", "find . -exec rm {} \\;",
	} {
		refusal, err := EvaluateRedirects(command, "", false, allowAll)
		if err != nil {
			t.Fatal(err)
		}
		if refusal.Refused {
			t.Errorf("%q refused with no scope bound: %+v", command, refusal)
		}
	}
}

// Dev posture still cannot reach a credential through any of them.
func TestArgumentWritesToSecretsAreHardUnderDevPosture(t *testing.T) {
	for name, template := range scopedWriteCommands {
		command := strings.ReplaceAll(template, "TARGET", ".env")
		g := newGate(t, map[string]string{flags.HarnessPosture: flags.PostureDev})
		d, err := g.EvaluateCommand(command, scopedWriteTask())
		if err != nil {
			t.Fatal(err)
		}
		if !d.Blocked() || d.Rule != policy.RuleSecretPath || d.Severity != policy.Hard {
			t.Errorf("%s: %q = %s/%s/%s, want hard %s", name, command, d.Action, d.Rule, d.Severity, policy.RuleSecretPath)
		}
	}
}
