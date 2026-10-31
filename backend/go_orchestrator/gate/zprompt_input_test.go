package gate

import (
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/flags"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/policy"
)

// The composed form: through the gate, under the postures that switch the hard
// rules off or demote scope, a write to a prompt input is still refused, and so
// is the shell redirect that performs the same write.
func TestPromptInputsAreRefusedUnderEveryPosture(t *testing.T) {
	for _, posture := range []string{flags.PostureStrict, flags.PostureDev, flags.PostureYolo} {
		g := newGate(t, map[string]string{flags.HarnessPosture: posture})
		task := testTask()
		task.PlannedFiles = append(task.PlannedFiles, dc.PlannedFile{Path: "**", AllowedChange: dc.ChangeModify})
		for _, name := range []string{"AGENTS.md", "CLAUDE.md", "GEMINI.md", "mcp.json", ".mcp.json", "docs/AGENTS.md",
			".agents/skills/x/SKILL.md", ".claude/skills/x/SKILL.md", ".cursor/skills/x/SKILL.md"} {
			d, err := g.EvaluateWrite(name, task, dc.OpWrite)
			if err != nil {
				t.Fatal(err)
			}
			if !d.Blocked() || d.Rule != policy.RuleRestrictedPath || d.Overridable() {
				t.Errorf("%s: write %q = %+v, want a hard %s", posture, name, d, policy.RuleRestrictedPath)
			}
			if r, err := g.EvaluateRead(name, task); err != nil || r.Blocked() {
				t.Errorf("%s: read %q = %+v (%v), want allowed", posture, name, r, err)
			}
			cmd, err := g.EvaluateCommand("echo x > "+name, task)
			if err != nil {
				t.Fatal(err)
			}
			if !cmd.Blocked() || cmd.Rule != policy.RuleRestrictedPath {
				t.Errorf("%s: `echo x > %s` = %+v, want blocked by %s", posture, name, cmd, policy.RuleRestrictedPath)
			}
		}
	}
}
