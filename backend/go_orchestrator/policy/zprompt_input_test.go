package policy

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
)

// The five names an agent must not be able to write: three are the next turn's
// system prompt, two declare commands the harness executes.
var promptInputNames = []string{"AGENTS.md", "CLAUDE.md", "GEMINI.md", "mcp.json", ".mcp.json"}

func promptInputTask(paths ...string) *dc.Task {
	task := &dc.Task{ID: "T"}
	for _, p := range paths {
		task.PlannedFiles = append(task.PlannedFiles, dc.PlannedFile{Path: p, AllowedChange: dc.ChangeModify})
	}
	return task
}

// Refused under every posture: the rung is outside the hard-rules block, so a
// gate with HardRules off (the yolo posture) must refuse exactly as one with it
// on, and a task that plans the file cannot authorise it.
func TestPromptInputsAreNeverWritable(t *testing.T) {
	for _, hard := range []bool{true, false} {
		gate := FileGate{Root: t.TempDir(), HardRules: hard}
		for _, name := range promptInputNames {
			for _, task := range []*dc.Task{nil, promptInputTask("src/a.go"), promptInputTask(name, "**")} {
				for _, op := range []dc.Operation{dc.OpWrite, dc.OpDelete} {
					d := gate.EvaluateFileChange(name, task, op, false)
					if d.Action != Deny || d.Rule != RuleRestrictedPath || d.Severity != Hard {
						t.Errorf("hard=%v %s %q task=%v = %s/%s/%s, want hard %s",
							hard, op, name, task != nil, d.Action, d.Rule, d.Severity, RuleRestrictedPath)
					}
					if d.Overridable() {
						t.Errorf("hard=%v %q: a prompt-input refusal must not be grantable", hard, name)
					}
				}
			}
		}
	}
}

// Nested files are refused too: the working directory is a subdirectory when an
// agent is launched in one, and the other clients load nested instruction files.
func TestNestedPromptInputsAreNeverWritable(t *testing.T) {
	for _, hard := range []bool{true, false} {
		gate := FileGate{Root: t.TempDir(), HardRules: hard}
		for _, name := range []string{
			"docs/AGENTS.md", "pkg/sub/CLAUDE.md", "a/b/c/GEMINI.md", "plugins/x/mcp.json", "tools/.mcp.json",
			"./docs/AGENTS.md",
		} {
			d := gate.EvaluateFileChange(name, promptInputTask(name), dc.OpWrite, false)
			if d.Action != Deny || d.Rule != RuleRestrictedPath {
				t.Errorf("hard=%v write %q = %s/%s, want %s", hard, name, d.Action, d.Rule, RuleRestrictedPath)
			}
		}
	}
}

// APFS and NTFS fold case, so a case variant is the same file.
func TestPromptInputSpellingsFold(t *testing.T) {
	for _, hard := range []bool{true, false} {
		gate := FileGate{Root: t.TempDir(), HardRules: hard}
		for _, name := range []string{"agents.md", "Claude.MD", "GEMINI.md", "MCP.JSON", ".MCP.json", "docs/agents.MD"} {
			d := gate.EvaluateFileChange(name, promptInputTask(name), dc.OpWrite, false)
			if d.Action != Deny || d.Rule != RuleRestrictedPath {
				t.Errorf("hard=%v write %q = %s/%s, want %s", hard, name, d.Action, d.Rule, RuleRestrictedPath)
			}
		}
	}
}

// Whole names only: a file that merely contains one of the names is a file.
func TestPromptInputLookalikesStayWritable(t *testing.T) {
	for _, hard := range []bool{true, false} {
		gate := FileGate{Root: t.TempDir(), HardRules: hard}
		for _, name := range []string{
			"AGENTS.md.bak", "MYAGENTS.md", "AGENTS.mdx", "docs/CLAUDE.md.orig", "mcp.json5", "notmcp.json",
			"AGENTS.md/notes.txt", "src/a.go",
		} {
			d := gate.EvaluateFileChange(name, promptInputTask(name), dc.OpWrite, false)
			if d.Blocked() {
				t.Errorf("hard=%v write %q = %s/%s, want allowed", hard, name, d.Action, d.Rule)
			}
		}
	}
}

// Write-only: the instruction files are how an agent learns the project, so no
// read path may refuse them. This is why they are not in SecretPathPatterns.
func TestPromptInputsRemainReadable(t *testing.T) {
	root := t.TempDir()
	for _, hard := range []bool{true, false} {
		gate := FileGate{Root: root, HardRules: hard}
		for _, name := range append(append([]string{}, promptInputNames...), "docs/AGENTS.md") {
			if d := gate.EvaluateRead(name, nil); d.Blocked() {
				t.Errorf("hard=%v read %q = %s/%s, want allowed", hard, name, d.Action, d.Rule)
			}
		}
	}
	for _, name := range append(append([]string{}, promptInputNames...), "docs/AGENTS.md") {
		if got := ClassifyRead(root, name); got != ReadAllowed {
			t.Errorf("ClassifyRead(%q) = %v, want ReadAllowed", name, got)
		}
	}
}

// The internal path is how the host writes what it owns (a managed plane that
// materialises instructions); it must not be refused by a rule that exists to
// stop an agent.
func TestPromptInputsAreWritableInternally(t *testing.T) {
	for _, hard := range []bool{true, false} {
		gate := FileGate{Root: t.TempDir(), HardRules: hard}
		for _, name := range promptInputNames {
			if d := gate.EvaluateFileChange(name, promptInputTask(name), dc.OpWrite, true); d.Blocked() {
				t.Errorf("hard=%v internal write %q = %s/%s, want allowed", hard, name, d.Action, d.Rule)
			}
		}
	}
}

// With the hard rules off, an escaping path is the operator's choice and a file
// of the same name there is not this repository's prompt input.
func TestPromptInputRungIgnoresPathsOutsideTheRoot(t *testing.T) {
	root := t.TempDir()
	outside := filepath.Join(t.TempDir(), "AGENTS.md")
	gate := FileGate{Root: root, HardRules: false}
	if d := gate.EvaluateFileChange(outside, promptInputTask(), dc.OpWrite, false); d.Rule == RuleRestrictedPath {
		t.Errorf("outside write %q = %s/%s, want the prompt-input rung to stay out of it", outside, d.Action, d.Rule)
	}
}

// A symlink named innocently that resolves to an instruction file is that file.
func TestSymlinkToPromptInputIsRefused(t *testing.T) {
	root := t.TempDir()
	if err := os.WriteFile(filepath.Join(root, "AGENTS.md"), []byte("rules"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink("AGENTS.md", filepath.Join(root, "notes.txt")); err != nil {
		t.Skipf("symlinks unavailable: %v", err)
	}
	for _, hard := range []bool{true, false} {
		gate := FileGate{Root: root, HardRules: hard}
		d := gate.EvaluateFileChange("notes.txt", promptInputTask("notes.txt"), dc.OpWrite, false)
		if d.Action != Deny || d.Rule != RuleRestrictedPath {
			t.Errorf("hard=%v write through symlink = %s/%s, want %s", hard, d.Action, d.Rule, RuleRestrictedPath)
		}
	}
}

// Skills are prompt inputs too: a skill body is served to the model as
// instructions, and under yolo the restricted rung that also lists these
// directories is off, so an agent could author the skill it then loads.
func TestSkillRootsAreNeverWritable(t *testing.T) {
	for _, hard := range []bool{true, false} {
		gate := FileGate{Root: t.TempDir(), HardRules: hard}
		for _, name := range []string{
			".agents/skills/x/SKILL.md", ".agents/skills/new/SKILL.md", ".claude/skills/x/SKILL.md",
			".cursor/skills/x/SKILL.md", ".Agents/Skills/x/SKILL.md", "./.claude/skills/y.md",
			".agents/skills",
		} {
			for _, task := range []*dc.Task{nil, promptInputTask(name, "**")} {
				d := gate.EvaluateFileChange(name, task, dc.OpWrite, false)
				if d.Action != Deny || d.Rule != RuleRestrictedPath || d.Severity != Hard {
					t.Errorf("hard=%v write %q = %s/%s/%s, want hard %s", hard, name, d.Action, d.Rule, d.Severity, RuleRestrictedPath)
				}
			}
			if d := gate.EvaluateFileChange(name, promptInputTask(name), dc.OpWrite, true); d.Blocked() && d.Rule == RuleRestrictedPath {
				t.Errorf("hard=%v internal write %q refused as %s", hard, name, d.Rule)
			}
		}
	}
}

// Anchored at the root and whole components: lookalikes and nested directories of
// the same name are ordinary paths, and every skill file stays readable.
//
// The rest of the agent-config trees are NOT this list's business: with the hard
// rules off, .claude/settings.json stays writable, which is the yolo contract
// gate.TestYoloPostureAlsoDisablesHardRules pins. Widening this list to them is
// a decision for that contract's owner, so this pins today's boundary.
func TestSkillRootLookalikesAndOtherConfigStayWritableWithoutHardRules(t *testing.T) {
	root := t.TempDir()
	for _, hard := range []bool{true, false} {
		gate := FileGate{Root: root, HardRules: hard}
		for _, name := range []string{
			".claudeish/skills/x", ".agentsmd/x", "x.claude/skills/y", "src/claude/skills/x", "docs/agents/skills/x.md",
			".github/workflows/ci.yml",
		} {
			if d := gate.EvaluateFileChange(name, promptInputTask(name), dc.OpWrite, false); d.Blocked() {
				t.Errorf("hard=%v write %q = %s/%s, want allowed", hard, name, d.Action, d.Rule)
			}
		}
		if !hard {
			for _, name := range []string{".claude/settings.json", ".agents/other.md", ".agents/skills.md", "opencode.json"} {
				if d := gate.EvaluateFileChange(name, promptInputTask(name), dc.OpWrite, false); d.Blocked() {
					t.Errorf("hard=false write %q = %s/%s: the yolo contract leaves non-skill agent config writable", name, d.Action, d.Rule)
				}
			}
		}
		for _, name := range []string{".agents/skills/x/SKILL.md", ".claude/skills/x/SKILL.md"} {
			if d := gate.EvaluateRead(name, nil); d.Blocked() {
				t.Errorf("hard=%v read %q = %s/%s, want allowed", hard, name, d.Action, d.Rule)
			}
			if got := ClassifyRead(root, name); got != ReadAllowed {
				t.Errorf("ClassifyRead(%q) = %v", name, got)
			}
		}
	}
}

// Plugin declarations Manvi discovers (manvi/mcp/plugin.go) name commands it
// spawns. .devcouncil/plugins is also restricted, but only while the hard rules
// run, so it is the row that proves the new rung.
func TestPluginDeclarationsAreNeverWritable(t *testing.T) {
	for _, hard := range []bool{true, false} {
		gate := FileGate{Root: t.TempDir(), HardRules: hard}
		for _, name := range []string{
			"plugins/a/plugin.json", "plugins/a/b/openplugin.json", ".mcp/plugins/x/plugin.json",
			".devcouncil/plugins/p/openplugin.json", "Plugins/A/Plugin.JSON", "plugins/mcp.json",
		} {
			d := gate.EvaluateFileChange(name, promptInputTask(name, "**"), dc.OpWrite, false)
			if d.Action != Deny || d.Rule != RuleRestrictedPath {
				t.Errorf("hard=%v write %q = %s/%s, want %s", hard, name, d.Action, d.Rule, RuleRestrictedPath)
			}
		}
		for _, name := range []string{"src/plugin.json", "plugins/a/README.md", "docs/plugins/a/plugin.json"} {
			if d := gate.EvaluateFileChange(name, promptInputTask(name), dc.OpWrite, false); d.Blocked() {
				t.Errorf("hard=%v write %q = %s/%s, want allowed", hard, name, d.Action, d.Rule)
			}
		}
	}
}

// The Win32 path layer drops a component's trailing dots and spaces and reads
// "name::$DATA" as the file itself, so each of these is a name for a prompt input
// on Windows even though no byte of it matches. Refused as text, wherever the
// agent is running: the rung cannot ask the filesystem for a file not yet created.
func TestWindowsSpellingsOfPromptInputsAreRefused(t *testing.T) {
	for _, hard := range []bool{true, false} {
		gate := FileGate{Root: t.TempDir(), HardRules: hard}
		for _, name := range []string{
			"AGENTS.md.", "AGENTS.md..", "CLAUDE.md ", "AGENTS.md. .", "AGENTS.md::$DATA", "agents.md::$data",
			"docs/mcp.json.", ".mcp.json::$DATA", ".claude./skills/x/SKILL.md", ".agents/skills./x/SKILL.md",
			"plugins/a/plugin.json.",
		} {
			d := gate.EvaluateFileChange(name, promptInputTask(name, "**"), dc.OpWrite, false)
			if d.Action != Deny || (!hard && d.Rule != RuleRestrictedPath) {
				t.Errorf("hard=%v write %q = %s/%s, want a denial (%s without the hard rules)", hard, name, d.Action, d.Rule, RuleRestrictedPath)
			}
		}
	}
}

// Identity, not spelling: a write reaching the same file as a root-level prompt
// input is refused whatever the agent calls it.
func TestRootPromptInputIdentityIsRefused(t *testing.T) {
	root, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	for _, f := range []string{"AGENTS.md", "docs/rules.md", "src/a.go"} {
		p := filepath.Join(root, f)
		if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(p, []byte("x"), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	if err := os.Link(filepath.Join(root, "AGENTS.md"), filepath.Join(root, "hardlink.txt")); err != nil {
		t.Skipf("hard links unavailable: %v", err)
	}
	if err := os.Symlink("docs/rules.md", filepath.Join(root, "CLAUDE.md")); err != nil {
		t.Skipf("symlinks unavailable: %v", err)
	}
	for _, hard := range []bool{true, false} {
		gate := FileGate{Root: root, HardRules: hard}
		for _, name := range []string{"hardlink.txt", "docs/rules.md"} {
			d := gate.EvaluateFileChange(name, promptInputTask(name, "**"), dc.OpWrite, false)
			if d.Action != Deny || d.Rule != RuleRestrictedPath {
				t.Errorf("hard=%v write %q = %s/%s, want %s", hard, name, d.Action, d.Rule, RuleRestrictedPath)
			}
		}
		if d := gate.EvaluateFileChange("src/a.go", promptInputTask("src/a.go"), dc.OpWrite, false); d.Blocked() {
			t.Errorf("hard=%v an unrelated file was refused: %s/%s", hard, d.Action, d.Rule)
		}
		// Internal is the host's own writer and is not judged.
		if d := gate.EvaluateFileChange("hardlink.txt", promptInputTask("hardlink.txt"), dc.OpWrite, true); d.Blocked() {
			t.Errorf("hard=%v internal write = %s/%s, want allowed", hard, d.Action, d.Rule)
		}
	}
}

func redirectWritesOf(t *testing.T, command string) []string {
	t.Helper()
	set, err := RedirectWrites(command)
	if err != nil {
		t.Fatalf("RedirectWrites(%q): %v", command, err)
	}
	return set.Targets
}

// `ln` of a prompt input without -s makes a second writable name for it.
func TestHardLinkOfPromptInputIsJudgedAsAWrite(t *testing.T) {
	for _, cmd := range []string{"ln AGENTS.md notes.txt", "ln -f docs/CLAUDE.md x", "ln -t out AGENTS.md"} {
		w := redirectWritesOf(t, cmd)
		found := false
		for _, target := range w {
			if IsPromptInput(target) {
				found = true
			}
		}
		if !found {
			t.Errorf("%q: write targets %v do not include the prompt input it links", cmd, w)
		}
	}
	if w := redirectWritesOf(t, "ln -s AGENTS.md notes.txt"); len(w) != 1 || w[0] != "notes.txt" {
		t.Errorf("a symbolic link writes only its name, got %v", w)
	}
}

func TestIsPromptInput(t *testing.T) {
	for name, want := range map[string]bool{
		"AGENTS.md": true, "a/b/mcp.json": true, ".mcp.json": true, `docs\CLAUDE.md`: true,
		"AGENTS.md.bak": false, "": false, "a/": false, "src/mcp.json.bak": false, ".agents/skills/x": true, ".claude/settings.json": false,
		"src/.agents/skills/x": true, "plugins/a/plugin.json": true, ".mcp/plugins/a/openplugin.json": true,
		".devcouncil/plugins/a/plugin.json": true, "src/plugin.json": false, "plugins/a/notes.md": false,
		"AGENTS.md.": true, "agents.md ": true, "AGENTS.md::$DATA": true, ".claude./skills/x": true, "AGENTS.md:evil": false, ".agents/skills.md": false,
	} {
		if got := IsPromptInput(name); got != want {
			t.Errorf("IsPromptInput(%q) = %v, want %v", name, got, want)
		}
	}
}
