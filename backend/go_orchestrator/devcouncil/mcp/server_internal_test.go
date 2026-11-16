package mcp

import (
	"bufio"
	"bytes"
	"encoding/json"
	"io"
	"testing"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil"
)

func TestToolsListFailsOnInvalidSchema(t *testing.T) {
	req, err := json.Marshal(map[string]any{
		"jsonrpc": "2.0",
		"id":      1,
		"method":  "tools/list",
		"params":  map[string]any{},
	})
	if err != nil {
		t.Fatal(err)
	}
	pr, pw := io.Pipe()
	go func() {
		_, _ = pw.Write(append(req, '\n'))
		_ = pw.Close()
	}()
	var stdout bytes.Buffer
	srv := &Server{
		In:  pr,
		Out: &stdout,
		specsOverride: []devcouncil.ToolSpec{
			{
				Name:        "broken",
				InputSchema: json.RawMessage("{not-json"),
			},
		},
	}
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
	var resp rpcResponse
	if err := json.Unmarshal(sc.Bytes(), &resp); err != nil {
		t.Fatalf("unmarshal %q: %v", sc.Text(), err)
	}
	if resp.Error == nil || resp.Error.Code != -32603 {
		t.Fatalf("resp.Error = %+v, want -32603", resp.Error)
	}
}
