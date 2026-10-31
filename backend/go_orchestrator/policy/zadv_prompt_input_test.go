package policy

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
)

// Adversarial (dh-4-adversarial) attack matrix against the prompt-input rung,
// driven through FileGate on a real filesystem. Each row is a spelling or an
// aliasing route to a prompt input; want is what a sound rung does. A failing
// row is a BROKEN verdict, not a flaky test.

func advMust(t *testing.T, err error) {
	t.Helper()
	if err != nil {
		t.Fatal(err)
	}
}

func advWrite(t *testing.T, p, body string) {
	t.Helper()
	advMust(t, os.MkdirAll(filepath.Dir(p), 0o755))
	advMust(t, os.WriteFile(p, []byte(body), 0o644))
}

func advFixture(t *testing.T) string {
	t.Helper()
	root, err := filepath.EvalSymlinks(t.TempDir())
	advMust(t, err)
	advWrite(t, filepath.Join(root, "AGENTS.md"), "rules")
	advWrite(t, filepath.Join(root, "CLAUDE.md"), "rules")
	advWrite(t, filepath.Join(root, "docs", "rules.md"), "rules")
	advWrite(t, filepath.Join(root, "elsewhere", "x", "SKILL.md"), "skill")
	advWrite(t, filepath.Join(root, "src", "a.go"), "package a")
	advMust(t, os.Symlink(".", filepath.Join(root, "dotdocs")))
	advMust(t, os.Symlink("AGENTS.md", filepath.Join(root, "foo")))
	advMust(t, os.Link(filepath.Join(root, "AGENTS.md"), filepath.Join(root, "hl")))
	// A repository that keeps its instruction file elsewhere and links it in.
	advMust(t, os.Symlink("docs/rules.md", filepath.Join(root, "GEMINI.md")))
	// A skill root that is itself a link to another directory of the repo.
	advMust(t, os.MkdirAll(filepath.Join(root, ".agents"), 0o755))
	advMust(t, os.Symlink("../elsewhere", filepath.Join(root, ".agents", "skills")))
	// Dangling link named like a prompt input's alias.
	advMust(t, os.Symlink("MISSING/CLAUDE.md", filepath.Join(root, "dangling")))
	return root
}

func TestAdvPathSpellingsOfPromptInputs(t *testing.T) {
	root := advFixture(t)
	abs := filepath.Join(root, "AGENTS.md")
	rows := []struct {
		name string
		path string
		want bool // refused
		note string
	}{
		{"dot-slash", "./AGENTS.md", true, ""},
		{"dotdot-collapse", "a/../AGENTS.md", true, ""},
		{"trailing-slash", "AGENTS.md/", true, ""},
		{"trailing-dot", "AGENTS.md.", true, "windows strips the dot; refused only by the hard-rules malformed rung"},
		{"trailing-space", "AGENTS.md ", true, "cleanInput trims it, so the name is the protected one"},
		{"lower", "agents.md", true, ""},
		{"mixed", "Agents.MD", true, ""},
		{"abs-inside-root", abs, true, ""},
		{"dir-symlink", "dotdocs/AGENTS.md", true, "docs -> ."},
		{"dir-symlink-new", "dotdocs/newdir/AGENTS.md", true, "creation through a dir link"},
		{"file-symlink-to-prompt-input", "foo", true, "foo -> AGENTS.md"},
		{"dangling-symlink-resolves-to-name", "dangling", true, "dangling -> MISSING/CLAUDE.md"},
		{"hardlink-to-prompt-input", "hl", true, "hard link: same inode as AGENTS.md, no spelling in common"},
		{"prompt-input-is-a-symlink-to-other-file", "GEMINI.md", true, "GEMINI.md -> docs/rules.md; the logical name is protected"},
		{"target-of-prompt-input-symlink", "docs/rules.md", true, "GEMINI.md -> docs/rules.md: closed by identity (SameFile against the root-level prompt inputs), not by spelling"},
		{"skill-root-is-symlink", ".agents/skills/x/SKILL.md", true, ".agents/skills -> ../elsewhere"},
		{"skill-root-is-symlink-new-file", ".agents/skills/new/SKILL.md", true, "creation through the linked root"},
		{"nested-skill-dotdot", ".claude/skills/../skills/x/SKILL.md", true, ""},
		{"skill-root-itself", ".claude/skills", true, ""},
		{"nested-claude-skills", "sub/.claude/skills/x/SKILL.md", true, "skill roots at any depth, as the five names are"},
		{"other-agents-dir", ".agents/other/x.md", false, "intended: only skill roots (under hard rules the restricted rung refuses it too; see the loop)"},
		{"cyrillic-lookalike", "АGENTS.md", false, "a different file on every filesystem; not an alias"},
		{"long-s-fold", "AGENTſ.md", true, "U+017F folds to s; APFS treats it as AGENTS.md when the file exists"},
		{"nfd-ascii-noop", "AGENTS.md", true, ""},
	}
	for _, hard := range []bool{true, false} {
		gate := FileGate{Root: root, HardRules: hard}
		for _, r := range rows {
			d := gate.EvaluateFileChange(r.path, promptInputTask(r.path, "**"), dc.OpWrite, false)
			refused := d.Action == Deny && d.Rule == RuleRestrictedPath
			if hard && r.name == "other-agents-dir" {
				r.want = true // .agents/** is in RestrictedPathPatterns, which runs with the hard rules
			}
			if !hard && r.name == "trailing-dot" {
				// Under strict the malformed-path rung also refuses; judged separately below.
				t.Logf("hard=%v %-45s refused=%v rule=%s  (%s)", hard, r.name, refused, d.Rule, r.note)
			}
			if refused != r.want {
				// Under strict a different hard rule (path.malformed, path.restricted
				// for agent-config files) may refuse; that still counts as refused
				// for rows whose point is "an agent cannot write this".
				if hard && r.want && d.Action == Deny && d.Severity == Hard {
					continue
				}
				t.Errorf("BROKEN hard=%v %s: write %q = %s/%s, want refused=%v (%s)", hard, r.name, r.path, d.Action, d.Rule, r.want, r.note)
			}
		}
	}
}

// delete and create-where-absent are covered by the same rung.
func TestAdvDeleteAndCreate(t *testing.T) {
	root := advFixture(t)
	for _, hard := range []bool{true, false} {
		gate := FileGate{Root: root, HardRules: hard}
		for _, op := range []dc.Operation{dc.OpDelete, dc.OpCreate, dc.OpWrite} {
			for _, name := range []string{"AGENTS.md", "newdir/CLAUDE.md", ".mcp.json"} {
				d := gate.EvaluateFileChange(name, promptInputTask(name, "**"), op, false)
				if d.Action != Deny || d.Rule != RuleRestrictedPath {
					t.Errorf("BROKEN hard=%v op=%s %q = %s/%s", hard, op, name, d.Action, d.Rule)
				}
			}
		}
	}
}

// A directory that CONTAINS a prompt input is not itself one: replacing or
// removing the directory delivers/destroys the file without ever naming it.
func TestAdvParentDirectoryOfPromptInput(t *testing.T) {
	root := advFixture(t)
	for _, hard := range []bool{true, false} {
		gate := FileGate{Root: root, HardRules: hard}
		for _, name := range []string{".claude", ".agents", ".cursor", "docs"} {
			d := gate.EvaluateFileChange(name, promptInputTask(name, "**"), dc.OpWrite, false)
			t.Logf("hard=%v parent-dir write %q = %s/%s", hard, name, d.Action, d.Rule)
		}
	}
}

// The filesystem under this repository folds U+017F (long s) to s: creating AGENT\u017f.md creates AGENTS.md. With no AGENTS.md on disk yet, onDiskSpelling has nothing to match, so only fnmatch.Fold stands between the agent and a new prompt input.
func TestAdvLongSCreatesPromptInputWhenAbsent(t *testing.T) {
	for _, hard := range []bool{true, false} {
		gate := FileGate{Root: t.TempDir(), HardRules: hard}
		for _, name := range []string{"AGENT\u017f.md", "sub/CLAUDE.md", "CLAUDE.md", "\u017fkills/x", ".agents/\u017fkills/x/SKILL.md", "\u212a/x", ".mcp.json"} {
			d := gate.EvaluateFileChange(name, promptInputTask(name, "**"), dc.OpCreate, false)
			t.Logf("hard=%v create %q = %s/%s", hard, name, d.Action, d.Rule)
		}
		d := gate.EvaluateFileChange("AGENT\u017f.md", promptInputTask("**"), dc.OpCreate, false)
		if d.Action != Deny || d.Rule != RuleRestrictedPath {
			t.Errorf("BROKEN hard=%v: creating AGENT\u017f.md (an alias of AGENTS.md on APFS) = %s/%s", hard, d.Action, d.Rule)
		}
		d = gate.EvaluateFileChange(".agents/\u017fkills/x/SKILL.md", promptInputTask("**"), dc.OpCreate, false)
		if d.Action != Deny || d.Rule != RuleRestrictedPath {
			t.Errorf("BROKEN hard=%v: creating .agents/\u017fkills/x/SKILL.md = %s/%s", hard, d.Action, d.Rule)
		}
	}
}
