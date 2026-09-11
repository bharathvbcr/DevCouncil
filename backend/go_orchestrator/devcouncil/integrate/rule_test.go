package integrate

import (
	"strings"
	"testing"
)

// The rule must not tell an agent it is gated when nothing gates it. Hooks are
// retired and both gate switches default off, so a mandate here only produces
// agents that stop to ask for a lease before running a command.
func TestCursorRuleDoesNotMandateTheTaskLoop(t *testing.T) {
	for _, phrase := range []string{
		"do not guess task state",
		"Use DevCouncil MCP tools for status, checkout, scope, and verify",
	} {
		if strings.Contains(cursorRule, phrase) {
			t.Fatalf("the rule still mandates the task loop: %q", phrase)
		}
	}
	for _, phrase := range []string{
		"no task lease",
		"opt-in",
		"components and modules",
		"Manvi",
	} {
		if !strings.Contains(cursorRule, phrase) {
			t.Fatalf("the rule no longer says the task loop is optional: %q missing", phrase)
		}
	}
}

// Navigation stays directive. Softening the task loop must not soften this:
// an agent that guesses where a symbol lives answers from the wrong file.
func TestCursorRuleKeepsDevmapDirective(t *testing.T) {
	for _, phrase := range []string{
		"Navigate with DevMap before reading or grepping",
		"devmap_explore",
		"devmap_impact",
		"devmap_affected_tests",
		"impact analysis before editing",
		"truncated",
		"gaps.jsonl",
	} {
		if !strings.Contains(cursorRule, phrase) {
			t.Fatalf("DevMap guidance lost %q", phrase)
		}
	}
}

// The rule points only at commands that exist. `dev map query` was a Python
// verb; the Rust CLI has no `query` subcommand, so naming it sends an agent to
// a command that exits non-zero.
func TestCursorRuleNamesNoRetiredCommand(t *testing.T) {
	for _, retired := range []string{
		"dev map query",
		"dev graph query",
		"dev integrate cursor --apply",
		"GitNexus",
	} {
		if strings.Contains(cursorRule, retired) {
			t.Fatalf("the rule names a retired command or tool: %q", retired)
		}
	}
}

// `--destination` is relative to the project; the absolute root belongs on
// `--project-root`. Getting that backwards is refused by devmap, and integrate
// then reported success while installing nothing.
func TestSkillInstallNamesTheProjectRootNotTheDestination(t *testing.T) {
	args := skillInstallArgs("/repos/thing", "cursor")
	if len(args) < 2 || args[0] != "--project-root" || args[1] != "/repos/thing" {
		t.Fatalf("the repository must be passed as --project-root: %v", args)
	}
	for i, a := range args {
		if a == "--destination" {
			if i+1 >= len(args) {
				t.Fatalf("--destination without a value: %v", args)
			}
			if strings.HasPrefix(args[i+1], "/") {
				t.Fatalf("--destination must be project-relative, got %q", args[i+1])
			}
		}
	}
}

func TestSkillInstallPicksTheHostsSkillDirectory(t *testing.T) {
	cursor := strings.Join(skillInstallArgs("/r", "cursor"), " ")
	if !strings.Contains(cursor, "--destination .cursor/skills") {
		t.Fatalf("cursor: %s", cursor)
	}
	claude := strings.Join(skillInstallArgs("/r", "claude"), " ")
	if !strings.Contains(claude, "--destination .claude/skills") {
		t.Fatalf("claude: %s", claude)
	}
	// An unmapped host takes DevMap's own defaults rather than a guess.
	other := skillInstallArgs("/r", "codex")
	if len(other) != 2 {
		t.Fatalf("an unmapped host must name no destination: %v", other)
	}
}

func TestCursorRuleIsAValidAlwaysAppliedMdc(t *testing.T) {
	if !strings.HasPrefix(cursorRule, "---\n") {
		t.Fatal("an .mdc rule opens with front matter")
	}
	rest := cursorRule[4:]
	end := strings.Index(rest, "\n---\n")
	if end < 0 {
		t.Fatal("front matter is unterminated, so Cursor reads the whole rule as YAML")
	}
	front := rest[:end]
	if !strings.Contains(front, "alwaysApply: true") {
		t.Fatalf("front matter must set alwaysApply: %q", front)
	}
	if !strings.Contains(front, "description:") {
		t.Fatalf("front matter must carry a description: %q", front)
	}
}
