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
)

func TestToolsListMatchesRegistry(t *testing.T) {
	reg := devcouncil.NewRegistry(t.TempDir(), nil, nil)
	var stdin bytes.Buffer
	var stdout bytes.Buffer
	req := map[string]any{
		"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": map[string]any{},
	}
	b, _ := json.Marshal(req)
	stdin.Write(append(b, '\n'))
	// Close by returning EOF after one line: use a pipe.
	pr, pw := io.Pipe()
	go func() {
		_, _ = pw.Write(append(b, '\n'))
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
		t.Fatal("no response")
	}
	var resp struct {
		Result struct {
			Tools []struct {
				Name string `json:"name"`
			} `json:"tools"`
		} `json:"result"`
	}
	if err := json.Unmarshal(sc.Bytes(), &resp); err != nil {
		t.Fatal(err)
	}
	if len(resp.Result.Tools) != len(reg.Specs()) {
		t.Fatalf("list=%d specs=%d", len(resp.Result.Tools), len(reg.Specs()))
	}
}
