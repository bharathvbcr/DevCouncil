package integrate

import (
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/agents"
)

func TestAntigravitySendsAgentsBeforeTheMCPInstall(t *testing.T) {
	bin := devmapForTest(t)
	root := newRepo(t)
	receipt := apply(t, bin, root, "antigravity")
	if len(receipt.Spawned) < 2 ||
		!strings.Contains(receipt.Spawned[0], "agents install") ||
		!strings.Contains(receipt.Spawned[1], "servers-stdin") {
		t.Fatalf("agents install did not run before MCP integrate: %v", receipt.Spawned)
	}
	library, err := agents.Load()
	if err != nil {
		t.Fatal(err)
	}
	for _, file := range library.Agents {
		rel := filepath.Join(".agents", "agents", file.Name, "agent.md")
		body, err := os.ReadFile(filepath.Join(root, rel))
		if err != nil {
			t.Fatal(err)
		}
		if string(body) != string(file.Content) {
			t.Fatalf("%s bytes differ from the embedded library", rel)
		}
	}
	for _, file := range library.Rules {
		rel := filepath.Join(".agents", "rules", file.Name+".md")
		body, err := os.ReadFile(filepath.Join(root, rel))
		if err != nil {
			t.Fatal(err)
		}
		if string(body) != string(file.Content) {
			t.Fatalf("%s bytes differ from the embedded library", rel)
		}
	}
	check, err := Run(Options{Root: root, Host: "antigravity", Mode: ModeCheck, DevmapBin: bin, SelfBin: fakeSelf})
	if err != nil {
		t.Fatal(err)
	}
	for _, rel := range []string{
		".agents/agents/devcouncil/agent.md",
		".agents/agents/devcouncil-implementer/agent.md",
		".agents/agents/devcouncil-reviewer/agent.md",
		".agents/agents/devcouncil-verifier/agent.md",
		".agents/rules/devcouncil.md",
		".devcouncil-agents.json",
	} {
		if check.Files[rel] != "unchanged" {
			t.Errorf("check reports %s as %q\nfiles: %v", rel, check.Files[rel], check.Files)
		}
	}
}

func TestCursorDoesNotSpawnAgentsInstall(t *testing.T) {
	bin := devmapForTest(t)
	root := newRepo(t)
	receipt := apply(t, bin, root, "cursor")
	for _, line := range receipt.Spawned {
		if strings.Contains(line, "agents install") {
			t.Fatalf("cursor spawned agents install: %v", receipt.Spawned)
		}
	}
	if _, err := os.Stat(filepath.Join(root, ".agents", "agents")); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("cursor wrote .agents/agents: %v", err)
	}
}

func TestAntigravityDryRunWritesNoAgents(t *testing.T) {
	bin := devmapForTest(t)
	root := t.TempDir()
	receipt, err := Run(Options{Root: root, Host: "antigravity", Mode: ModeDryRun, DevmapBin: bin, SelfBin: fakeSelf})
	if err != nil {
		t.Fatal(err)
	}
	if len(receipt.Spawned) < 2 || !strings.Contains(receipt.Spawned[0], "agents install") {
		t.Fatalf("dry-run did not plan agents first: %v", receipt.Spawned)
	}
	if _, err := os.Stat(filepath.Join(root, ".agents", "agents")); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("dry-run wrote .agents/agents: %v", err)
	}
	if _, err := os.Stat(filepath.Join(root, ".devcouncil-agents.json")); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("dry-run wrote the agents receipt: %v", err)
	}
}

func TestAntigravityRuleDoesNotMandateALease(t *testing.T) {
	library, err := agents.Load()
	if err != nil {
		t.Fatal(err)
	}
	var rule string
	for _, file := range library.Rules {
		if file.Name == "devcouncil" {
			rule = string(file.Content)
		}
	}
	if rule == "" {
		t.Fatal("the library has no devcouncil rule")
	}
	for _, phrase := range []string{
		"do not guess task state",
		"Use DevCouncil MCP tools for status, checkout, scope, and verify",
	} {
		if strings.Contains(rule, phrase) {
			t.Fatalf("the rule still mandates the task loop: %q", phrase)
		}
	}
	for _, phrase := range []string{
		"no task lease",
		"opt-in",
		"Navigate with DevMap before reading or grepping",
		"devcouncil-implementer",
		"devcouncil-reviewer",
		"devcouncil-verifier",
	} {
		if !strings.Contains(rule, phrase) {
			t.Fatalf("the rule no longer says the task loop is optional: %q missing", phrase)
		}
	}
	if strings.Contains(rule, "alwaysOn") || !strings.Contains(rule, "trigger: always_on") {
		t.Fatalf("rule trigger is not always_on:\n%s", rule)
	}
}

func TestAgentsCheckThatCouldNotRunIsAnError(t *testing.T) {
	log := filepath.Join(t.TempDir(), "log")
	bin := writeDevmapScript(t, `#!/bin/sh
echo "$*" >> "$DEVMAP_LOG"
case " $* " in
  *" agents install "*)
    cat >/dev/null
    echo "agents install could not run" >&2
    exit 1
    ;;
esac
echo "integrate ran" >> "$DEVMAP_LOG"
printf '%s\n' '{"check_ok":true,"host":"antigravity"}'
exit 0
`)
	t.Setenv("DEVMAP_LOG", log)
	t.Setenv("HOME", t.TempDir())
	root := t.TempDir()
	receipt, err := Run(Options{Root: root, Host: "antigravity", Mode: ModeCheck, DevmapBin: bin, SelfBin: fakeSelf})
	if err == nil {
		t.Fatal("a check that could not run succeeded")
	}
	if receipt != nil {
		t.Fatalf("a failed check returned a receipt: %+v", receipt)
	}
	body, readErr := os.ReadFile(log)
	if readErr != nil {
		t.Fatal(readErr)
	}
	if strings.Contains(string(body), "integrate ran") {
		t.Fatalf("MCP install ran after agents could not be checked: %s", body)
	}
}

func TestAgentsThenMCPFailureKeepsTheReceipt(t *testing.T) {
	bin := writeDevmapScript(t, `#!/bin/sh
case " $* " in
  *" agents install "*)
    cat >/dev/null
    printf '%s\n' '{"written":["/opt/devcouncil-test/agent.md"],"differing":[],"managed":["/opt/devcouncil-test/agent.md"],"receipt":"/opt/devcouncil-test/.devcouncil-agents.json","check_ok":true}'
    exit 0
    ;;
  *" integrate "*)
    printf '%s\n' '{"error":"mcp document refused"}'
    exit 1
    ;;
esac
echo "unexpected $*" >&2
exit 1
`)
	t.Setenv("HOME", t.TempDir())
	root := t.TempDir()
	receipt, err := Run(Options{Root: root, Host: "antigravity", Mode: ModeApply, DevmapBin: bin, SelfBin: fakeSelf})
	if err == nil || !strings.Contains(err.Error(), "mcp document refused") {
		t.Fatalf("want the MCP refusal, got %v", err)
	}
	if receipt == nil {
		t.Fatal("agents succeeded but the error dropped the receipt")
	}
	if len(receipt.Spawned) == 0 || !strings.Contains(receipt.Spawned[0], "agents install") {
		t.Fatalf("spawned: %v", receipt.Spawned)
	}
	joined := strings.Join(append(receipt.Spawned, receipt.Notes...), " ")
	if strings.Contains(joined, "uninstalled") {
		t.Fatalf("the receipt describes the agents as uninstalled: %+v", receipt)
	}
	if receipt.Files["/opt/devcouncil-test/agent.md"] != "wrote" {
		t.Fatalf("written agent path: %v", receipt.Files)
	}
}

func writeDevmapScript(t *testing.T, script string) string {
	t.Helper()
	path := filepath.Join(t.TempDir(), "devmap")
	if err := os.WriteFile(path, []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	return path
}
