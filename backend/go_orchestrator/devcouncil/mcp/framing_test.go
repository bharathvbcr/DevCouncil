package mcp_test

import (
	"bytes"
	"encoding/json"
	"io"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/mcp"
)

// syncBuffer is an io.Writer a test can read while the server still writes.
type syncBuffer struct {
	mu  sync.Mutex
	buf bytes.Buffer
}

func (b *syncBuffer) Write(p []byte) (int, error) {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.buf.Write(p)
}

func (b *syncBuffer) lines() []string {
	b.mu.Lock()
	defer b.mu.Unlock()
	var out []string
	for _, l := range strings.Split(b.buf.String(), "\n") {
		if strings.TrimSpace(l) != "" {
			out = append(out, l)
		}
	}
	return out
}

// serveFrames feeds frames to a server, closes stdin, and returns every line it
// wrote once Serve has returned.
func serveFrames(t *testing.T, frames ...string) []string {
	t.Helper()
	reg := devcouncil.NewRegistry(t.TempDir(), nil, nil)
	out := &syncBuffer{}
	srv := &mcp.Server{Registry: reg, In: strings.NewReader(strings.Join(frames, "\n") + "\n"), Out: out, Err: io.Discard}
	done := make(chan error, 1)
	go func() { done <- srv.Serve() }()
	select {
	case <-done:
	case <-time.After(10 * time.Second):
		t.Fatal("Serve did not return")
	}
	return out.lines()
}

type reply struct {
	ID     json.RawMessage `json:"id"`
	Result json.RawMessage `json:"result"`
	Error  *struct {
		Code    int    `json:"code"`
		Message string `json:"message"`
	} `json:"error"`
}

func decodeReply(t *testing.T, line string) reply {
	t.Helper()
	var r reply
	if err := json.Unmarshal([]byte(line), &r); err != nil {
		t.Fatalf("not a response object: %q: %v", line, err)
	}
	return r
}

// Every frame that carries an id gets exactly one response carrying that id.
// These are the shapes the Go host used to leave unanswered or answer with a
// missing id; devmap's own MCP server (rust/devmap-serve/src/mcp.rs,
// dispatch_single) answers each the way this table says.
func TestEveryFrameWithAnIDGetsOneResponseCarryingIt(t *testing.T) {
	cases := []struct {
		name     string
		frame    string
		wantID   string
		wantCode int // 0 means a result
	}{
		{"jsonrpc of the wrong type", `{"jsonrpc":2,"id":1,"method":"ping"}`, `1`, -32600},
		{"jsonrpc of the wrong version", `{"jsonrpc":"1.0","id":"a","method":"ping"}`, `"a"`, -32600},
		{"method of the wrong type", `{"jsonrpc":"2.0","id":2,"method":7}`, `2`, -32600},
		{"no method", `{"jsonrpc":"2.0","id":3}`, `3`, -32600},
		{"fractional id", `{"jsonrpc":"2.0","id":1.5,"method":"ping"}`, `1.5`, -32600},
		{"null id", `{"jsonrpc":"2.0","id":null,"method":"ping"}`, `null`, -32600},
		{"notification method carrying an id", `{"jsonrpc":"2.0","id":4,"method":"notifications/initialized"}`, `4`, 0},
		{"cancel notification carrying an id", `{"jsonrpc":"2.0","id":5,"method":"notifications/cancelled","params":{"requestId":99}}`, `5`, 0},
		{"unknown method", `{"jsonrpc":"2.0","id":6,"method":"nope"}`, `6`, -32601},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			lines := serveFrames(t, tc.frame)
			if len(lines) != 1 {
				t.Fatalf("got %d responses, want exactly 1: %q", len(lines), lines)
			}
			r := decodeReply(t, lines[0])
			if string(r.ID) != tc.wantID {
				t.Errorf("response id = %s, want %s: %s", r.ID, tc.wantID, lines[0])
			}
			switch {
			case tc.wantCode == 0 && (r.Error != nil || r.Result == nil):
				t.Errorf("want a result: %s", lines[0])
			case tc.wantCode != 0 && (r.Error == nil || r.Error.Code != tc.wantCode):
				t.Errorf("want error %d: %s", tc.wantCode, lines[0])
			}
		})
	}
}

func TestFramesThatAreNotRequestsAreAnsweredWithANullID(t *testing.T) {
	for name, frame := range map[string]string{
		"not json":        `{"jsonrpc":`,
		"a bare number":   `7`,
		"a bare string":   `"ping"`,
		"empty batch":     `[]`,
		"nested batch":    `[[{"jsonrpc":"2.0","id":1,"method":"ping"}]]`,
		"non-object item": `[7]`,
	} {
		t.Run(name, func(t *testing.T) {
			lines := serveFrames(t, frame)
			if len(lines) != 1 {
				t.Fatalf("got %d responses, want 1: %q", len(lines), lines)
			}
			if !strings.Contains(lines[0], `"id":null`) {
				t.Errorf("response must carry an explicit null id: %s", lines[0])
			}
			if strings.HasPrefix(lines[0], "[") {
				return // a batch member's fault is reported inside the array
			}
			r := decodeReply(t, lines[0])
			want := -32600
			if name == "not json" {
				want = -32700
			}
			if r.Error == nil || r.Error.Code != want {
				t.Errorf("want error %d: %s", want, lines[0])
			}
		})
	}
}

func TestNotificationsGetNoResponse(t *testing.T) {
	lines := serveFrames(t,
		`{"jsonrpc":"2.0","method":"notifications/initialized"}`,
		`{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}}`,
		`{"jsonrpc":"2.0","method":"tools/list"}`,
		`{"jsonrpc":"2.0","method":"ping"}`)
	if len(lines) != 0 {
		t.Fatalf("notifications were answered: %q", lines)
	}
}

func TestABatchIsAnsweredWithAnArrayOfItsRequestsOnly(t *testing.T) {
	lines := serveFrames(t, `[{"jsonrpc":"2.0","id":10,"method":"ping"},{"jsonrpc":"2.0","method":"notifications/initialized"},{"jsonrpc":"2.0","id":"b","method":"nope"}]`)
	if len(lines) != 1 {
		t.Fatalf("got %d lines, want one array: %q", len(lines), lines)
	}
	var batch []reply
	if err := json.Unmarshal([]byte(lines[0]), &batch); err != nil {
		t.Fatalf("not an array: %s: %v", lines[0], err)
	}
	if len(batch) != 2 || string(batch[0].ID) != "10" || batch[0].Error != nil || string(batch[1].ID) != `"b"` || batch[1].Error == nil || batch[1].Error.Code != -32601 {
		t.Fatalf("batch answer = %s", lines[0])
	}
	if lines := serveFrames(t, `[{"jsonrpc":"2.0","method":"notifications/initialized"}]`); len(lines) != 0 {
		t.Fatalf("a batch of notifications was answered: %q", lines)
	}
}

func TestAnOversizeFrameGetsAReplyAndTheNextFrameStillRuns(t *testing.T) {
	huge := `{"jsonrpc":"2.0","id":1,"method":"ping","params":{"pad":"` + strings.Repeat("x", 5<<20) + `"}}`
	lines := serveFrames(t, huge, `{"jsonrpc":"2.0","id":2,"method":"ping"}`)
	if len(lines) != 2 {
		t.Fatalf("got %d responses, want 2: %.200q", len(lines), lines)
	}
	first := decodeReply(t, lines[0])
	if first.Error == nil || first.Error.Code != -32600 || !strings.Contains(first.Error.Message, "frame") {
		t.Errorf("oversize reply = %s", lines[0])
	}
	if second := decodeReply(t, lines[1]); string(second.ID) != "2" || second.Error != nil {
		t.Errorf("the frame after an oversize one was not served: %s", lines[1])
	}
}
