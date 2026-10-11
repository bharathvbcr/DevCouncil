package mcp_test

import (
	"bufio"
	"bytes"
	"encoding/json"
	"io"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/mcp"
)

// exchange sends one request to a fresh server and returns its decoded
// response.
func exchange(t *testing.T, srv *mcp.Server, request any) map[string]any {
	t.Helper()
	frame, err := json.Marshal(request)
	if err != nil {
		t.Fatal(err)
	}
	var stdout bytes.Buffer
	pr, pw := io.Pipe()
	go func() {
		_, _ = pw.Write(append(frame, '\n'))
		_ = pw.Close()
	}()
	srv.Registry = devcouncil.NewRegistry(t.TempDir(), nil, nil)
	srv.In = pr
	srv.Out = &stdout
	done := make(chan error, 1)
	go func() { done <- srv.Serve() }()
	select {
	case err := <-done:
		if err != nil && err != io.EOF {
			t.Fatal(err)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("timeout")
	}
	sc := bufio.NewScanner(&stdout)
	sc.Buffer(make([]byte, 0, 1<<20), 8<<20)
	if !sc.Scan() {
		t.Fatalf("no response to %s", frame)
	}
	var resp map[string]any
	if err := json.Unmarshal(sc.Bytes(), &resp); err != nil {
		t.Fatal(err)
	}
	return resp
}

// at walks a dotted path ("result.supportedVersions.0") through decoded JSON.
func at(doc any, path string) (any, bool) {
	cur := doc
	for _, part := range strings.Split(path, ".") {
		switch node := cur.(type) {
		case map[string]any:
			next, ok := node[part]
			if !ok {
				return nil, false
			}
			cur = next
		case []any:
			i, err := strconv.Atoi(part)
			if err != nil || i < 0 || i >= len(node) {
				return nil, false
			}
			cur = node[i]
		default:
			return nil, false
		}
	}
	return cur, true
}

// TestNegotiationTranscript replays protocols/mcp/negotiation.v1.json, the
// transcript both the Go and the Rust MCP servers are held to.
func TestNegotiationTranscript(t *testing.T) {
	raw, err := os.ReadFile(filepath.Join("..", "..", "..", "..", "protocols", "mcp", "negotiation.v1.json"))
	if err != nil {
		t.Fatal(err)
	}
	var transcript struct {
		Cases []struct {
			Name      string         `json:"name"`
			Request   map[string]any `json:"request"`
			ErrorCode *int           `json:"error_code"`
			Expect    map[string]any `json:"expect"`
		} `json:"cases"`
	}
	if err := json.Unmarshal(raw, &transcript); err != nil {
		t.Fatal(err)
	}
	if len(transcript.Cases) == 0 {
		t.Fatal("transcript has no cases")
	}
	for _, c := range transcript.Cases {
		t.Run(c.Name, func(t *testing.T) {
			resp := exchange(t, &mcp.Server{}, c.Request)
			if c.ErrorCode != nil {
				got, _ := at(resp, "error.code")
				if got != float64(*c.ErrorCode) {
					t.Fatalf("error.code = %v, want %d; response %v", got, *c.ErrorCode, resp)
				}
			} else if _, failed := resp["error"]; failed {
				t.Fatalf("unexpected error: %v", resp["error"])
			}
			for path, want := range c.Expect {
				got, ok := at(resp, path)
				if !ok || got != want {
					t.Fatalf("%s = %v (present %v), want %v; response %v", path, got, ok, want, resp)
				}
			}
		})
	}
}

func initialize(version string) map[string]any {
	params := map[string]any{"capabilities": map[string]any{}}
	if version != "" {
		params["protocolVersion"] = version
	}
	return map[string]any{"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": params}
}

// The default is a choice, not a pin: an unrecognised or absent version is
// answered per DefaultProtocol, and a version the client names that this
// server supports is always honoured.
func TestDefaultProtocolIsConfigurable(t *testing.T) {
	cases := []struct {
		name, def, asked, wantVersion string
		wantError                     bool
	}{
		{"legacy default counter-offers the newest handshake revision", "legacy", "1999-01-01", "2025-11-25", false},
		{"unset default is legacy", "", "", "2025-11-25", false},
		{"explicit handshake default is the counter-offer", "2025-03-26", "1999-01-01", "2025-03-26", false},
		{"a supported client version beats any default", "2025-03-26", "2025-06-18", "2025-06-18", false},
		{"modern default refuses an unknown handshake version rather than counter-offering legacy", "modern", "1999-01-01", "", true},
		{"modern default still serves a client that names a supported handshake version", "modern", "2024-11-05", "2024-11-05", false},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			resp := exchange(t, &mcp.Server{DefaultProtocol: c.def}, initialize(c.asked))
			if c.wantError {
				if _, ok := at(resp, "error.data.supported.0"); !ok {
					t.Fatalf("want an error naming supported versions, got %v", resp)
				}
				return
			}
			got, _ := at(resp, "result.protocolVersion")
			if got != c.wantVersion {
				t.Fatalf("protocolVersion = %v, want %s", got, c.wantVersion)
			}
		})
	}
}

func TestEnvDefaultProtocolIsHonouredAndValidated(t *testing.T) {
	t.Setenv("DEVCOUNCIL_MCP_PROTOCOL_DEFAULT", "2025-06-18")
	resp := exchange(t, &mcp.Server{}, initialize("1999-01-01"))
	if got, _ := at(resp, "result.protocolVersion"); got != "2025-06-18" {
		t.Fatalf("env default not honoured: %v", resp)
	}
	t.Setenv("DEVCOUNCIL_MCP_PROTOCOL_DEFAULT", "nonsense")
	resp = exchange(t, &mcp.Server{}, initialize("1999-01-01"))
	if got, _ := at(resp, "result.protocolVersion"); got != "2025-11-25" {
		t.Fatalf("an unrecognised default must fall back to legacy, got %v", resp)
	}
}

func TestModernEraIsStatelessAndServerInfoIsStamped(t *testing.T) {
	req := map[string]any{"jsonrpc": "2.0", "id": 7, "method": "server/discover", "params": map[string]any{}}
	resp := exchange(t, &mcp.Server{}, req)
	if got, _ := at(resp, "result.cacheScope"); got != "private" {
		t.Fatalf("discover result: %v", resp)
	}
	modern := map[string]any{"jsonrpc": "2.0", "id": 8, "method": "tools/list", "params": map[string]any{"_meta": map[string]any{
		"io.modelcontextprotocol/protocolVersion":    "2026-07-28",
		"io.modelcontextprotocol/clientCapabilities": map[string]any{},
	}}}
	resp = exchange(t, &mcp.Server{}, modern)
	meta, _ := at(resp, "result._meta")
	m, _ := meta.(map[string]any)
	info, _ := m["io.modelcontextprotocol/serverInfo"].(map[string]any)
	if info["name"] != "devcouncil" {
		t.Fatalf("modern result must carry serverInfo in _meta, got %v", resp["result"])
	}
}
