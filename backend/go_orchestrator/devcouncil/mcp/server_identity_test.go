package mcp_test

import (
	"bufio"
	"bytes"
	"encoding/json"
	"io"
	"testing"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/mcp"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/version"
)

// `initialize` is the only place a host learns which DevCouncil it is talking
// to, and nothing asserted it: the field was a literal `"0.1.0"` from the
// first release through 0.2.3, so every Claude Code, Cursor and Codex host
// that ever handshook with this server was told the wrong build. The value is
// compared against `version.Version` rather than against a spelled-out number
// so that this test tracks the product instead of becoming a second place to
// edit on a release.
func TestInitializeReportsTheProductVersion(t *testing.T) {
	reg := devcouncil.NewRegistry(t.TempDir(), nil, nil)
	request, err := json.Marshal(map[string]any{
		"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": map[string]any{},
	})
	if err != nil {
		t.Fatal(err)
	}

	var stdout bytes.Buffer
	pr, pw := io.Pipe()
	go func() {
		_, _ = pw.Write(append(request, '\n'))
		_ = pw.Close()
	}()
	srv := &mcp.Server{Registry: reg, In: pr, Out: &stdout}
	done := make(chan error, 1)
	go func() { done <- srv.Serve() }()
	select {
	case err := <-done:
		if err != nil && err != io.EOF {
			t.Fatal(err)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("timeout")
	}

	sc := bufio.NewScanner(&stdout)
	if !sc.Scan() {
		t.Fatal("no response to initialize")
	}
	var resp struct {
		Result struct {
			ServerInfo struct {
				Name    string `json:"name"`
				Version string `json:"version"`
			} `json:"serverInfo"`
		} `json:"result"`
	}
	if err := json.Unmarshal(sc.Bytes(), &resp); err != nil {
		t.Fatal(err)
	}
	if resp.Result.ServerInfo.Name != "devcouncil" {
		t.Fatalf("serverInfo.name = %q, want %q", resp.Result.ServerInfo.Name, "devcouncil")
	}
	if resp.Result.ServerInfo.Version != version.Version {
		t.Fatalf(
			"serverInfo.version = %q, want %q; the MCP server is announcing a build that is not this one",
			resp.Result.ServerInfo.Version, version.Version,
		)
	}
}
