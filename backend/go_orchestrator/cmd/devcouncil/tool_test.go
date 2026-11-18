//go:build unix

package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/gatescfg"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/mcp"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/gate"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// toolRepo is a git repository with a task store holding one ready task,
// T-1, whose scope plans a.go. dcstore is on PATH, as an installed host has it.
func toolRepo(t *testing.T) string {
	t.Helper()
	bin := testsupport.DCStore(t)
	t.Setenv("PATH", filepath.Dir(bin)+string(os.PathListSeparator)+os.Getenv("PATH"))
	root := t.TempDir()
	git := testsupport.Tool(t, "git")
	for _, args := range [][]string{
		{"init", "-q"}, {"config", "user.email", "t@example.com"}, {"config", "user.name", "t"},
	} {
		runIn(t, root, git, args...)
	}
	if err := os.WriteFile(filepath.Join(root, "a.go"), []byte("package a\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	runIn(t, root, git, "add", "a.go")
	runIn(t, root, git, "commit", "-q", "-m", "init")
	if err := os.MkdirAll(filepath.Join(root, ".devcouncil"), 0o755); err != nil {
		t.Fatal(err)
	}
	scope, _ := json.Marshal(map[string]any{"planned_files": []string{"a.go"}})
	testsupport.SeedTask(t, gatescfg.DefaultWorkbenchDB(root), testsupport.TaskSeed{
		ID: "T-1", Title: "one", Status: "ready", Scope: string(scope),
	})
	return root
}

func runIn(t *testing.T, dir, bin string, args ...string) {
	t.Helper()
	cmd := exec.Command(bin, args...)
	cmd.Dir = dir
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("%s %v: %v\n%s", bin, args, err, out)
	}
}

// cliTool runs `devcouncil tool call` in this process and returns its stdout
// (without the trailing newline), stderr and exit status.
func cliTool(t *testing.T, root, name string, args map[string]any) (string, string, int) {
	t.Helper()
	argv := []string{"tool", "call", name, "--project-root", root}
	if args != nil {
		raw, err := json.Marshal(args)
		if err != nil {
			t.Fatal(err)
		}
		argv = append(argv, "--args", string(raw))
	}
	stdout, restoreOut := swapStdout(t)
	stderr, restoreErr := swapStderr(t)
	code := dispatch(argv)
	restoreOut()
	restoreErr()
	return strings.TrimSuffix(stdout.String(), "\n"), stderr.String(), code
}

// lockedBuffer is an io.Writer the server goroutine writes while the test
// reads.
type lockedBuffer struct {
	mu  sync.Mutex
	buf bytes.Buffer
}

func (b *lockedBuffer) Write(p []byte) (int, error) {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.buf.Write(p)
}

func (b *lockedBuffer) lines() []string {
	b.mu.Lock()
	defer b.mu.Unlock()
	return strings.Split(b.buf.String(), "\n")
}

// mcpTool asks a real MCP server, built over this root as runMCP builds it,
// for one tools/call, and returns the reply's text and isError.
func mcpTool(t *testing.T, root, name string, args map[string]any) (string, bool) {
	t.Helper()
	var g *gate.Gate
	if name == "devcouncil_policy_check_write" {
		built, err := openGate(root)
		if err != nil {
			t.Fatalf("openGate: %v", err)
		}
		g = built
	}
	client := openStore(root)
	if client != nil {
		defer client.Close()
	}
	pr, pw := io.Pipe()
	out := &lockedBuffer{}
	srv := &mcp.Server{Registry: devcouncil.NewRegistry(root, client, g), In: pr, Out: out, Err: io.Discard}
	done := make(chan error, 1)
	go func() { done <- srv.Serve() }()
	defer func() {
		_ = pw.Close()
		<-done
	}()
	params, _ := json.Marshal(map[string]any{"name": name, "arguments": args})
	for _, frame := range []string{
		`{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}`,
		`{"jsonrpc":"2.0","method":"notifications/initialized"}`,
		fmt.Sprintf(`{"jsonrpc":"2.0","id":2,"method":"tools/call","params":%s}`, params),
	} {
		if _, err := io.WriteString(pw, frame+"\n"); err != nil {
			t.Fatal(err)
		}
	}
	deadline := time.Now().Add(2 * time.Minute)
	for time.Now().Before(deadline) {
		for _, line := range out.lines() {
			var reply struct {
				ID     json.RawMessage `json:"id"`
				Result struct {
					Content []struct {
						Text string `json:"text"`
					} `json:"content"`
					IsError bool `json:"isError"`
				} `json:"result"`
				Error json.RawMessage `json:"error"`
			}
			if json.Unmarshal([]byte(line), &reply) != nil || string(reply.ID) != strconv.Itoa(2) {
				continue
			}
			if len(reply.Error) > 0 || len(reply.Result.Content) != 1 {
				t.Fatalf("%s over MCP: unexpected reply %s", name, line)
			}
			return reply.Result.Content[0].Text, reply.Result.IsError
		}
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatalf("%s over MCP: no reply within 2m", name)
	return "", false
}

// TestToolCallAnswersEveryServedToolAsMCPDoes holds the shell to the server
// for every tool this host serves: same name and arguments, same text, and an
// exit status that agrees with isError. A tool added to the registry without a
// row here fails the test, so the fallback cannot quietly lag the server.
//
// The calls are ones whose answers do not depend on when they were made (a
// lease token or an expiry would differ between two honest calls); the lease
// round trip itself is TestToolCallLeaseIsTheStoresNotTheServers.
func TestToolCallAnswersEveryServedToolAsMCPDoes(t *testing.T) {
	root := toolRepo(t)
	calls := map[string]map[string]any{
		"devcouncil_get_diff":           {"paths": []any{"a.go"}},
		"devcouncil_checkout_task":      {"task_id": "NOPE", "client_id": "parity"},
		"devcouncil_renew_lease":        {"task_id": "T-1", "lease_token": "not-a-token"},
		"devcouncil_release_task":       {"task_id": "T-1", "lease_token": "not-a-token"},
		"devcouncil_next_task":          {},
		"devcouncil_verify_task":        {"task_id": "T-1", "lease_token": "not-a-token"},
		"devcouncil_get_gaps":           {"task_id": "T-1"},
		"devcouncil_policy_check_write": {"path": "a.go", "task_id": "T-1", "operation": "modify"},
	}
	for _, name := range devcouncil.ServedToolNames() {
		args, ok := calls[name]
		if !ok {
			t.Errorf("%s is served but has no parity row here; add one", name)
			continue
		}
		t.Run(name, func(t *testing.T) {
			want, isError := mcpTool(t, root, name, args)
			got, stderr, code := cliTool(t, root, name, args)
			if got != want {
				t.Errorf("shell and MCP disagree for %s\nshell: %s\nmcp:   %s", name, got, want)
			}
			if isError != (code != devcouncil.ToolExitOK) {
				t.Errorf("%s: isError=%v over MCP but the shell exited %d", name, isError, code)
			}
			if !strings.Contains(stderr, "not over MCP") {
				t.Errorf("%s: the shell did not say it bypassed MCP; stderr=%q", name, stderr)
			}
		})
	}
	for name := range calls {
		if !servedTool(name) {
			t.Errorf("parity row %s names a tool this host does not serve", name)
		}
	}
}

// TestToolCallLeaseIsTheStoresNotTheServers takes a lease from the shell and
// uses it from an MCP server, then the other way round. The two sides share no
// process state, only the task store, so this is what proves a shell checkout
// is honoured where MCP looks.
func TestToolCallLeaseIsTheStoresNotTheServers(t *testing.T) {
	root := toolRepo(t)
	decode := func(text string) map[string]any {
		t.Helper()
		var v map[string]any
		if err := json.Unmarshal([]byte(text), &v); err != nil {
			t.Fatalf("not JSON: %v: %s", err, text)
		}
		return v
	}

	text, _, code := cliTool(t, root, "devcouncil_checkout_task", map[string]any{"task_id": "T-1", "client_id": "agent-a"})
	if code != 0 {
		t.Fatalf("shell checkout exit %d: %s", code, text)
	}
	token, _ := decode(text)["lease_token"].(string)
	if token == "" {
		t.Fatalf("shell checkout returned no lease_token: %s", text)
	}

	// MCP sees the shell's lease: another client is refused, the holder renews.
	text, isError := mcpTool(t, root, "devcouncil_checkout_task", map[string]any{"task_id": "T-1", "client_id": "agent-b"})
	if !isError || decode(text)["code"] != "lease_conflict" {
		t.Fatalf("MCP checkout by another client over a shell lease: %s", text)
	}
	text, isError = mcpTool(t, root, "devcouncil_renew_lease", map[string]any{"task_id": "T-1", "lease_token": token})
	if isError {
		t.Fatalf("MCP renew of the shell's lease: %s", text)
	}
	// The same client asking again, from MCP, is handed its lease back.
	text, isError = mcpTool(t, root, "devcouncil_checkout_task", map[string]any{"task_id": "T-1", "client_id": "agent-a"})
	if isError || decode(text)["lease_token"] != token {
		t.Fatalf("MCP checkout by the holding client did not hand the lease back: %s", text)
	}

	// And the shell releases what it took.
	text, _, code = cliTool(t, root, "devcouncil_release_task", map[string]any{"task_id": "T-1", "lease_token": token})
	if code != 0 {
		t.Fatalf("shell release exit %d: %s", code, text)
	}
	text, isError = mcpTool(t, root, "devcouncil_checkout_task", map[string]any{"task_id": "T-1", "client_id": "agent-b"})
	if isError {
		t.Fatalf("MCP checkout after the shell released: %s", text)
	}
	mcpToken, _ := decode(text)["lease_token"].(string)
	// The reverse: a lease MCP took refuses another shell client.
	text, _, code = cliTool(t, root, "devcouncil_checkout_task", map[string]any{"task_id": "T-1", "client_id": "agent-c"})
	if code != devcouncil.ToolExitRefused || decode(text)["code"] != "lease_conflict" {
		t.Fatalf("shell checkout over an MCP lease: exit %d %s", code, text)
	}
	text, _, code = cliTool(t, root, "devcouncil_renew_lease", map[string]any{"task_id": "T-1", "lease_token": mcpToken})
	if code != 0 {
		t.Fatalf("shell renew of MCP's lease: exit %d %s", code, text)
	}
}

func TestToolCallExitStatusSeparatesRefusalUsageAndNotRun(t *testing.T) {
	root := toolRepo(t)
	bare := t.TempDir() // no task store: every store tool cannot run here

	for _, tc := range []struct {
		what string
		argv []string
		want int
	}{
		{"unknown tool", []string{"tool", "call", "devcouncil_write_file", "--project-root", root}, devcouncil.ToolExitUsage},
		{"no tool named", []string{"tool", "call", "--project-root", root}, devcouncil.ToolExitUsage},
		{"args not JSON", []string{"tool", "call", "devcouncil_get_gaps", "--args", "{task_id:T-1}", "--project-root", root}, devcouncil.ToolExitUsage},
		{"args not an object", []string{"tool", "call", "devcouncil_get_gaps", "--args", "[\"T-1\"]", "--project-root", root}, devcouncil.ToolExitUsage},
		{"args twice", []string{"tool", "call", "devcouncil_get_gaps", "--args", "{}", "--args", "{}", "--project-root", root}, devcouncil.ToolExitUsage},
		{"positional args", []string{"tool", "call", "devcouncil_get_gaps", "task_id=T-1", "--project-root", root}, devcouncil.ToolExitUsage},
		{"undeclared argument", []string{"tool", "call", "devcouncil_verify_task", "--args", `{"task_id":"T-1","lease_token":"x","sandbox":"docker"}`, "--project-root", root}, devcouncil.ToolExitUsage},
		{"missing argument", []string{"tool", "call", "devcouncil_renew_lease", "--args", `{"task_id":"T-1"}`, "--project-root", root}, devcouncil.ToolExitUsage},
		{"unknown task", []string{"tool", "call", "devcouncil_get_gaps", "--args", `{"task_id":"NOPE"}`, "--project-root", root}, devcouncil.ToolExitRefused},
		{"no task store", []string{"tool", "call", "devcouncil_next_task", "--project-root", bare}, devcouncil.ToolExitNotRun},
		{"no task store, gaps", []string{"tool", "call", "devcouncil_get_gaps", "--args", `{"task_id":"T-1"}`, "--project-root", bare}, devcouncil.ToolExitNotRun},
		{"answered", []string{"tool", "call", "devcouncil_next_task", "--project-root", root}, devcouncil.ToolExitOK},
		{"answered, null args", []string{"tool", "call", "devcouncil_next_task", "--args", "null", "--project-root", root}, devcouncil.ToolExitOK},
	} {
		stdout, restoreOut := swapStdout(t)
		stderr, restoreErr := swapStderr(t)
		code := dispatch(tc.argv)
		restoreOut()
		restoreErr()
		if code != tc.want {
			t.Errorf("%s: exit %d, want %d\nstdout=%s\nstderr=%s", tc.what, code, tc.want, stdout.String(), stderr.String())
		}
	}
}

func TestToolCallReadsArgsFromStdin(t *testing.T) {
	got, err := parseToolArgs("-", true, strings.NewReader(`{"task_id":"T-1"}`))
	if err != nil || got["task_id"] != "T-1" {
		t.Fatalf("parseToolArgs(-) = %v, %v", got, err)
	}
	if _, err := parseToolArgs("-", true, strings.NewReader(strings.Repeat(" ", maxToolArgsBytes+1))); err == nil {
		t.Fatal("an oversized stdin was accepted")
	}
	if got, err := parseToolArgs("", false, strings.NewReader("ignored")); err != nil || len(got) != 0 {
		t.Fatalf("no --args read stdin or failed: %v, %v", got, err)
	}
}

func TestToolListNamesExactlyTheServedTools(t *testing.T) {
	stdout, restore := swapStdout(t)
	code := dispatch([]string{"tool", "list", "--json"})
	restore()
	if code != 0 {
		t.Fatalf("tool list --json exit %d", code)
	}
	var listed struct {
		Tools []struct {
			Name        string         `json:"name"`
			InputSchema map[string]any `json:"inputSchema"`
		} `json:"tools"`
	}
	if err := json.Unmarshal(stdout.Bytes(), &listed); err != nil {
		t.Fatalf("tool list --json: %v: %s", err, stdout.String())
	}
	served := devcouncil.ServedToolNames()
	if len(listed.Tools) != len(served) {
		t.Fatalf("listed %d tools, the host serves %d", len(listed.Tools), len(served))
	}
	for i, tool := range listed.Tools {
		if tool.Name != served[i] || tool.InputSchema == nil {
			t.Errorf("tool %d: listed %q with schema %v, served %q", i, tool.Name, tool.InputSchema, served[i])
		}
	}
}
