package main

import (
	"regexp"
	"sort"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/skills"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/policy"
)

// These tests hold everything that tells an agent what to run to the two
// surfaces that decide whether it can: the dispatch table (`commands`) and the
// MCP tools the registry serves. The policy allowlist, its refusals, the
// verifier's next actions and the packaged skills each named `dev status`,
// `dev checkout`, `dev doctor`, `dev scope update` or a retired devcouncil_*
// tool after the Python host was gone — every one of them an exit 2 or an
// unknown tool for the agent that followed the advice.

// invocationRe reads the command an agent would type at the start of a code
// span: `dev <cmd>` or `devcouncil <cmd>`, with an optional `uv run` that the
// test refuses outright, since no `uv` entry point for the host exists.
var invocationRe = regexp.MustCompile(`^(uv run (?:-\S+ \S+ )*)?(dev|devcouncil)\s+([^\s|&;>]+)`)

// toolRe finds devcouncil_* MCP tool names anywhere in a text.
var toolRe = regexp.MustCompile(`devcouncil_[a-z][a-z0-9_]*`)

// commandInvocations returns each `dev`/`devcouncil` invocation inside the
// given code fragments, one per shell segment.
func commandInvocations(fragments []string) []string {
	var out []string
	for _, fragment := range fragments {
		for _, segment := range regexp.MustCompile(`&&|\|\||[;|\n]`).Split(fragment, -1) {
			segment = strings.TrimPrefix(strings.TrimSpace(segment), "$ ")
			if invocationRe.MatchString(segment) {
				out = append(out, segment)
			}
		}
	}
	return out
}

// codeFragments returns the inline backtick spans and fenced code blocks of a
// markdown or plain text. Prose is not read: "a dev schema" is English, not a
// command, while `dev schema` is an instruction.
func codeFragments(text string) []string {
	var out []string
	var prose strings.Builder
	inFence := false
	var fence strings.Builder
	for _, line := range strings.Split(text, "\n") {
		if strings.HasPrefix(strings.TrimSpace(line), "```") {
			if inFence {
				out = append(out, fence.String())
				fence.Reset()
			}
			inFence = !inFence
			continue
		}
		if inFence {
			fence.WriteString(line + "\n")
			continue
		}
		prose.WriteString(line + "\n")
	}
	parts := strings.Split(prose.String(), "`")
	for i := 1; i < len(parts); i += 2 {
		out = append(out, parts[i])
	}
	return out
}

// deadInvocations returns each invocation that names no dispatched command.
func deadInvocations(invocations []string) []string {
	var dead []string
	for _, inv := range invocations {
		m := invocationRe.FindStringSubmatch(inv)
		if m == nil {
			continue
		}
		if m[1] != "" {
			dead = append(dead, inv+" (no `uv` entry point runs the host)")
			continue
		}
		if _, live := commands[m[3]]; !live {
			dead = append(dead, inv)
		}
	}
	return dead
}

func TestPolicyNamesOnlyLiveCommands(t *testing.T) {
	// The allowlists themselves. A pattern is its own invocation.
	lists := map[string][]string{
		"NoTaskAllowedCommands":         policy.NoTaskAllowedCommands,
		"LeaseLifecycleAllowedCommands": policy.LeaseLifecycleAllowedCommands,
	}
	for name, list := range lists {
		var named []string
		for _, pattern := range list {
			if strings.HasPrefix(pattern, "dev") || strings.HasPrefix(pattern, "uv run dev") ||
				strings.HasPrefix(pattern, "uv run -") {
				named = append(named, pattern)
			}
		}
		for _, dead := range deadInvocations(commandInvocations(named)) {
			t.Errorf("%s allows %q, which the host does not dispatch", name, dead)
		}
	}

	// The refusals, read from real decisions rather than from the source.
	task := &dc.Task{ID: "TASK-1", PlannedFiles: []dc.PlannedFile{{Path: "a/planned.go"}}}
	reasons := map[string]string{
		"no lease": policy.CommandGate{HardRules: true}.EvaluateCommand("curl http://example.com", nil).Reason,
		"unplanned, other directory": policy.FileGate{Root: "/repo", HardRules: true, AllowNeighbors: true, AllowSameDir: true}.
			EvaluateFileChange("b/other.go", task, dc.OpModify, false).Reason,
		"unplanned, repository root": policy.FileGate{Root: "/repo", HardRules: true, AllowNeighbors: true, AllowSameDir: true}.
			EvaluateFileChange("root.go", task, dc.OpModify, false).Reason,
	}
	for what, reason := range reasons {
		if reason == "" {
			t.Fatalf("%s: the decision carried no reason", what)
		}
		for _, dead := range deadInvocations(commandInvocations(codeFragments(reason))) {
			t.Errorf("the %s refusal tells the agent to run %q, which the host does not dispatch: %q",
				what, dead, reason)
		}
		for _, tool := range toolRe.FindAllString(reason, -1) {
			if !served(tool) {
				t.Errorf("the %s refusal names %s, which this host does not serve", what, tool)
			}
		}
	}
}

func TestNextActionsNameOnlyLiveCommands(t *testing.T) {
	file := "pkg/changed.go"
	line := 3
	types := verify.GapTypes()
	if len(types) == 0 {
		t.Fatal("verify.GapTypes is empty; nothing was checked")
	}
	var texts []string
	for _, gapType := range types {
		texts = append(texts, verify.NextActionFor(verify.Gap{
			ID: "G", GapType: gapType, File: &file, Line: &line,
			Evidence: []string{"symbol: Thing"},
		}).Action)
	}
	planned := []dc.PlannedFile{{Path: "pkg/planned.go"}}
	for _, g := range verify.DetectOrphanDiffGaps("TASK-1", planned,
		[]string{"pkg/changed.go", "pkg/new_test.go"}, map[string]struct{}{"pkg/new_test.go": {}}) {
		texts = append(texts, g.RecommendedFix)
	}
	for _, text := range texts {
		for _, dead := range deadInvocations(commandInvocations(codeFragments(text))) {
			t.Errorf("a next action tells the agent to run %q, which the host does not dispatch: %q", dead, text)
		}
	}
}

// TestPackagedSkillsNameOnlyLiveToolsAndCommands scans every skill the binary
// embeds — the library `skills scaffold` installs into each host — for MCP tool
// names and `dev` invocations, and refuses any this host does not have.
//
// It does not check `dev map <subcommand>`: those are DevMap's commands, whose
// table lives in another binary.
func TestPackagedSkillsNameOnlyLiveToolsAndCommands(t *testing.T) {
	all, err := skills.Embedded.Load()
	if err != nil {
		t.Fatal(err)
	}
	if len(all) == 0 {
		t.Fatal("the embedded skill library is empty; nothing was checked")
	}
	invocations := 0
	for _, skill := range all {
		text := string(skill.Content)
		for _, tool := range toolRe.FindAllString(text, -1) {
			if !served(tool) {
				t.Errorf("skill %s names %s, which this host does not serve", skill.Name, tool)
			}
		}
		named := commandInvocations(codeFragments(text))
		invocations += len(named)
		for _, dead := range deadInvocations(named) {
			t.Errorf("skill %s tells the agent to run %q, which the host does not dispatch", skill.Name, dead)
		}
		// No slash commands ship with this repository: nothing tracked
		// defines a `commands/` directory and integrate writes none.
		if strings.Contains(text, "/devcouncil:") {
			t.Errorf("skill %s names a /devcouncil:* slash command; none ships", skill.Name)
		}
	}
	if invocations == 0 {
		t.Fatal("no `dev` invocation was found in any skill; the scanner is reading nothing")
	}
}

// The scanner must see what it is meant to refuse, or a green run means
// nothing. Each of these is a shape the skills and refusals actually used.
func TestLivenessScannerCatchesTheRetiredShapes(t *testing.T) {
	text := "Run `dev status` or `uv run dev map`, then\n```bash\ndev repair TASK-1 # fix\n" +
		"dev verify TASK-1\n```\nand `cd x && dev doctor`. A dev schema is prose."
	got := deadInvocations(commandInvocations(codeFragments(text)))
	sort.Strings(got)
	want := []string{"dev doctor", "dev repair TASK-1 # fix", "dev status",
		"uv run dev map (no `uv` entry point runs the host)"}
	if strings.Join(got, "\n") != strings.Join(want, "\n") {
		t.Fatalf("dead invocations = %q, want %q", got, want)
	}
	if served("devcouncil_status") || !served("devcouncil_verify_task") {
		t.Fatal("served() does not read the registry's tool list")
	}
}

func served(tool string) bool {
	for _, name := range devcouncil.ServedToolNames() {
		if name == tool {
			return true
		}
	}
	return false
}
