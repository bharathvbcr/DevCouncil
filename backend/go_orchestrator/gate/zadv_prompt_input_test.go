package gate

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/flags"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/grants"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/policy"
)

// Adversarial (dh-4-adversarial): which tools reach a prompt input, per posture.
// Every row is judged through Gate.EvaluateCommand, the entry a host calls for a
// shell command. A row that comes back unblocked is a way to modify a prompt
// input that the new rung does not see.

func advGateFixture(t *testing.T, posture string) (*Gate, *dc.Task) {
	t.Helper()
	g := newGate(t, map[string]string{flags.HarnessPosture: posture})
	root, err := filepath.EvalSymlinks(g.Root)
	if err != nil {
		t.Fatal(err)
	}
	g.Root = root
	for _, f := range []string{"AGENTS.md", "CLAUDE.md", "p.patch", "stage/AGENTS.md", "stage/x.txt", "x.txt"} {
		p := filepath.Join(root, f)
		if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(p, []byte("x"), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	task := testTask()
	task.PlannedFiles = append(task.PlannedFiles, dc.PlannedFile{Path: "**", AllowedChange: dc.ChangeModify},
		dc.PlannedFile{Path: "**", AllowedChange: dc.ChangeDelete})
	// A task that allows the writers under test, so the verdict is the write
	// rungs' and not command.not_allowed's. The strict-without-allowlist case is
	// reported separately.
	task.AllowedCommands = []string{"python3 *", "perl *", "git *", "sh *", "dd *", "install *", "rsync *", "ed *",
		"cp *", "mv *", "tee *", "sed *", "ln *", "rm *", "tar *", "unzip *", "awk *", "node *", "ruby *", "echo *", "cat *", "printf *", "patch *", "truncate *"}
	return g, task
}

var advCommands = []struct {
	name string
	cmd  string
	// modelled is whether DevCouncil's writer model is expected to know the tool.
	modelled bool
}{
	{"redirect", "echo x > AGENTS.md", true},
	{"append", "echo x >> AGENTS.md", true},
	{"tee", "echo x | tee AGENTS.md", true},
	{"sed-i", "sed -i.bak s/a/b/ AGENTS.md", true},
	{"cp", "cp x.txt AGENTS.md", true},
	{"cp-into-dir", "cp x.txt stage/", false},
	{"mv-onto", "mv x.txt AGENTS.md", true},
	{"mv-away", "mv AGENTS.md y.txt", true},
	{"mv-dir-with-prompt-input-into-place", "mv stage docs2", false},
	{"cp-r-dir-with-prompt-input", "cp -r stage docs3", false},
	{"install", "install x.txt AGENTS.md", true},
	{"install-d", "install -D x.txt sub/CLAUDE.md", true},
	{"rsync", "rsync x.txt AGENTS.md", true},
	{"rsync-dir", "rsync -a stage/ docs4/", false},
	{"dd", "dd if=x.txt of=AGENTS.md", true},
	{"perl-i", "perl -pi -e s/a/b/ AGENTS.md", true},
	{"ln-s", "ln -sf x.txt AGENTS.md", true},
	{"ln-hardlink-new-name", "ln AGENTS.md hl2", true},
	{"rm", "rm AGENTS.md", true},
	{"rm-rf-parent-claude", "rm -rf .claude", true},
	{"truncate", "truncate -s0 AGENTS.md", true},
	{"python3-c", `python3 -c "open('AGENTS.md','w').write('x')"`, false},
	{"python3-script-name", "python3 evil.py", false},
	{"sh-c", `sh -c "echo x > AGENTS.md"`, false},
	{"node-e", `node -e "require('fs').writeFileSync('AGENTS.md','x')"`, false},
	{"ed", "ed AGENTS.md", false},
	{"awk-inplace", "awk -i inplace 1 AGENTS.md", false},
	{"git-checkout-path", "git checkout -- AGENTS.md", true},
	{"git-checkout-lone-path", "git checkout AGENTS.md", true},
	{"git-checkout-ref-path", "git checkout HEAD~1 AGENTS.md", true},
	{"git-checkout-branch", "git checkout evil-branch", false},
	{"git-restore", "git restore AGENTS.md", true},
	{"git-reset-hard", "git reset --hard HEAD~1", false},
	{"git-stash-pop", "git stash pop", false},
	{"git-apply", "git apply p.patch", true},
	{"git-am", "git am p.patch", false},
	{"git-merge", "git merge evil", false},
	{"git-pull", "git pull", false},
	{"git-cherry-pick", "git cherry-pick abc", false},
	{"git-mv", "git mv x.txt AGENTS.md", true},
	{"git-rm", "git rm AGENTS.md", true},
	{"patch", "patch AGENTS.md p.patch", true},
	{"tar-x", "tar xf e.tar", false},
	{"unzip", "unzip e.zip", false},
}

// residual names the rows that are documented residuals, not defects, and says
// where they hold (policy.PromptInputPatterns states the same boundary):
//
//	"yolo": refused under strict and dev by the unmodelled-program operand rung,
//	        which runs only with the hard rules, so open under yolo.
//	"all":  open under every posture. The command is not one the gate models
//	        (an interpreter, `sh -c`, an archive extractor, a git subcommand that
//	        rewrites the tree) or carries a prompt input inside a directory it
//	        copies, moves or syncs without naming the file; a task whose allowlist
//	        admits such a program has been handed a write capability no path rule
//	        sees. rm -rf of a skill root's parent is listed "yolo": it can destroy
//	        a prompt input but not author one, and every file it could recreate is
//	        refused.
//
// A residual row asserts the boundary: if one starts being blocked, the test
// fails so the documentation is updated rather than silently out of date.
var residual = map[string]string{
	"ed": "yolo", "awk-inplace": "yolo", "rm-rf-parent-claude": "yolo",
	"cp-into-dir": "all", "mv-dir-with-prompt-input-into-place": "all", "cp-r-dir-with-prompt-input": "all",
	"rsync-dir": "all", "python3-c": "all", "python3-script-name": "all", "sh-c": "all", "node-e": "all",
	"git-checkout-branch": "all", "git-reset-hard": "all", "git-stash-pop": "all", "git-am": "all",
	"git-merge": "all", "git-pull": "all", "git-cherry-pick": "all", "tar-x": "all", "unzip": "all",
}

func TestAdvCommandMatrix(t *testing.T) {
	for _, posture := range []string{flags.PostureStrict, flags.PostureDev, flags.PostureYolo} {
		g, task := advGateFixture(t, posture)
		for _, c := range advCommands {
			d, err := g.EvaluateCommand(c.cmd, task)
			if err != nil {
				t.Fatal(err)
			}
			t.Logf("MATRIX posture=%-6s %-38s blocked=%-5v rule=%-28s sev=%v  modelled=%v", posture, c.name, d.Blocked(), d.Rule, d.Severity, c.modelled)
			open := residual[c.name] == "all" || (residual[c.name] == "yolo" && posture == flags.PostureYolo)
			switch {
			case open && d.Blocked():
				t.Errorf("RESIDUAL CLOSED posture=%s %q (%s) is now blocked (%s): update the documented residuals", posture, c.cmd, c.name, d.Rule)
			case !open && !d.Blocked():
				t.Errorf("BROKEN posture=%s %q (%s) was not blocked: %+v", posture, c.cmd, c.name, d)
			}
		}
	}
}

// What strict does with the same unmodelled programs when the task does NOT
// allowlist them: the general command allowlist should catch them there, making
// the gap a dev/yolo one. Records, does not assert.
func TestAdvStrictWithoutAllowlist(t *testing.T) {
	for _, posture := range []string{flags.PostureStrict, flags.PostureDev, flags.PostureYolo} {
		g, task := advGateFixture(t, posture)
		task.AllowedCommands = nil
		for _, c := range advCommands {
			if c.modelled {
				continue
			}
			d, err := g.EvaluateCommand(c.cmd, task)
			if err != nil {
				t.Fatal(err)
			}
			t.Logf("NOALLOW posture=%-6s %-38s blocked=%-5v rule=%-28s sev=%v", posture, c.name, d.Blocked(), d.Rule, d.Severity)
		}
	}
}

// Demotion and grants: the new rung's denial must be hard, ungrantable and
// undemotable under every posture, through both write and command entries.
func TestAdvPromptInputDenialIsNotDemotableOrGrantable(t *testing.T) {
	for _, posture := range []string{flags.PostureStrict, flags.PostureDev, flags.PostureYolo} {
		for _, mode := range []string{flags.ModeEnforce, flags.ModeAdvisory, flags.ModeOff} {
			g := newGate(t, map[string]string{flags.HarnessPosture: posture, flags.PolicyFileMode: mode, flags.PolicyCommandMode: mode})
			_, task := advGateFixture(t, posture)
			for _, auth := range []grants.Authority{grants.Human, grants.Agent} {
				_, err := g.Ledger.Issue(grants.Request{
					Grantor:   grants.Grantor{Authority: auth, ID: "adv"},
					Reason:    "adversarial",
					Scope:     grants.Scope{TaskID: task.ID, Rules: []policy.RuleID{policy.RuleRestrictedPath}},
					TTL:       time.Minute,
					AgentTask: task.ID,
				})
				if err == nil {
					t.Errorf("BROKEN %s/%s: a %s grant of path.restricted was issued", posture, mode, auth)
				}
			}
			for _, name := range []string{"AGENTS.md", "pkg/.mcp.json", ".claude/skills/x/SKILL.md"} {
				d, err := g.EvaluateWrite(name, task, dc.OpWrite)
				if err != nil {
					t.Fatal(err)
				}
				if !d.Blocked() || d.Severity != policy.Hard || d.Overridable() || d.Demoted != "" {
					t.Errorf("BROKEN %s/%s %s: %+v", posture, mode, name, d)
				}
				c, err := g.EvaluateCommand("echo x > "+name, task)
				if err != nil {
					t.Fatal(err)
				}
				if !c.Blocked() || c.Demoted != "" {
					t.Errorf("BROKEN %s/%s command redirect to %s: %+v", posture, mode, name, c)
				}
			}
		}
	}
}

// Reads stay open under every posture and gate mode: Gate.EvaluateRead,
// ClassifyRead-based filters, and the read-only commands an agent uses to load
// the instruction files.
func TestAdvPromptInputsStayReadable(t *testing.T) {
	for _, posture := range []string{flags.PostureStrict, flags.PostureDev, flags.PostureYolo} {
		g, task := advGateFixture(t, posture)
		for _, name := range []string{"AGENTS.md", "CLAUDE.md", "GEMINI.md", ".mcp.json", "mcp.json", ".claude/skills/x/SKILL.md", ".agents/skills/x/SKILL.md"} {
			if r, err := g.EvaluateRead(name, task); err != nil || r.Blocked() {
				t.Errorf("BROKEN %s: read %q = %+v (%v)", posture, name, r, err)
			}
			for _, cmd := range []string{"cat " + name, "head -5 " + name, "git show HEAD:" + name, "git diff HEAD -- " + name, "grep -n x " + name, "rg x " + name, "wc -l " + name} {
				c, err := g.EvaluateCommand(cmd, task)
				if err != nil {
					t.Fatal(err)
				}
				// Compare with the same command on an ordinary file: a read of a
				// prompt input must not be treated worse than a read of src/a.go.
				ref, _ := g.EvaluateCommand(cmdFor(cmd, name, "src/calc.go"), task)
				if c.Blocked() != ref.Blocked() {
					t.Errorf("BROKEN %s: %q blocked=%v (%s) but the same command on src/calc.go blocked=%v", posture, cmd, c.Blocked(), c.Rule, ref.Blocked())
				}
			}
		}
	}
}

func cmdFor(cmd, from, to string) string { return strings.ReplaceAll(cmd, from, to) }
