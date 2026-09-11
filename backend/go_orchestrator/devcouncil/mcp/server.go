package mcp

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"sync"
	"sync/atomic"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil"
)

const (
	maxFrameBytes     = 4 * 1024 * 1024
	maxInFlight       = 32
	defaultCallBudget = 60 * time.Second
	callBudgetEnv     = "DEVCOUNCIL_MCP_CALL_TIMEOUT_MS"
	watchdogTick      = 100 * time.Millisecond
	slotPoll          = 2 * time.Millisecond
	maxResponseBytes  = 4 * 1024 * 1024
)

// Server is a hand-rolled JSON-RPC MCP stdio server.
type Server struct {
	Registry *devcouncil.Registry
	In       io.Reader
	Out      io.Writer
	Err      io.Writer

	inFlight atomic.Int32
	broken   atomic.Bool
	outMu    sync.Mutex
}

type rpcRequest struct {
	JSONRPC string          `json:"jsonrpc"`
	ID      json.RawMessage `json:"id"`
	Method  string          `json:"method"`
	Params  json.RawMessage `json:"params"`
}

type rpcResponse struct {
	JSONRPC string          `json:"jsonrpc"`
	ID      json.RawMessage `json:"id,omitempty"`
	Result  any             `json:"result,omitempty"`
	Error   *rpcError       `json:"error,omitempty"`
}

type rpcError struct {
	Code    int    `json:"code"`
	Message string `json:"message"`
	Data    any    `json:"data,omitempty"`
}

type toolsCallParams struct {
	Name      string         `json:"name"`
	Arguments map[string]any `json:"arguments"`
}

func callBudget() time.Duration {
	if raw := os.Getenv(callBudgetEnv); raw != "" {
		var ms int64
		if _, err := fmt.Sscanf(raw, "%d", &ms); err == nil && ms > 0 {
			return time.Duration(ms) * time.Millisecond
		}
	}
	return defaultCallBudget
}

// Serve runs until stdin closes or the stdout pipe breaks.
// Both stdin and any child stderr must be drained by the caller; this loop
// reads stdin to EOF and never leaves a half-read frame buffering forever.
func (s *Server) Serve() error {
	if s.In == nil {
		s.In = os.Stdin
	}
	if s.Out == nil {
		s.Out = os.Stdout
	}
	if s.Err == nil {
		s.Err = os.Stderr
	}
	reader := bufio.NewReaderSize(s.In, 64*1024)
	pending := &sync.Map{} // id-string -> *pendingCall
	go s.watchdog(pending)

	for {
		if s.broken.Load() {
			s.drainInFlight()
			return io.ErrClosedPipe
		}
		line, err := readFrame(reader, maxFrameBytes)
		if err == io.EOF {
			s.drainInFlight()
			return nil
		}
		if err != nil {
			fmt.Fprintf(s.Err, "devcouncil-mcp: frame error: %v\n", err)
			if err == errFrameTooLarge {
				continue
			}
			s.drainInFlight()
			return err
		}
		if len(bytes.TrimSpace(line)) == 0 {
			continue
		}
		var req rpcRequest
		if err := json.Unmarshal(line, &req); err != nil {
			_ = s.send(rpcResponse{
				JSONRPC: "2.0",
				ID:      nil,
				Error:   &rpcError{Code: -32700, Message: "parse error"},
			})
			continue
		}
		if req.Method == "notifications/initialized" || req.Method == "notifications/cancelled" {
			continue
		}
		if len(req.ID) == 0 || string(req.ID) == "null" {
			continue // notification
		}

		for s.inFlight.Load() >= maxInFlight {
			if s.broken.Load() {
				return io.ErrClosedPipe
			}
			time.Sleep(slotPoll)
		}
		s.inFlight.Add(1)
		answered := &atomic.Bool{}
		idKey := string(req.ID)
		deadline := time.Now().Add(callBudget())
		pending.Store(idKey, &pendingCall{
			deadline: deadline,
			answered: answered,
			timeout: rpcResponse{
				JSONRPC: "2.0",
				ID:      req.ID,
				Error:   &rpcError{Code: -32000, Message: "request timed out"},
			},
		})
		go func(req rpcRequest) {
			defer s.inFlight.Add(-1)
			defer pending.Delete(string(req.ID))
			resp := s.handle(req)
			if answered.CompareAndSwap(false, true) {
				_ = s.send(resp)
			}
		}(req)
	}
}

type pendingCall struct {
	deadline time.Time
	answered *atomic.Bool
	timeout  rpcResponse
}

func (s *Server) watchdog(pending *sync.Map) {
	for {
		time.Sleep(watchdogTick)
		if s.broken.Load() {
			return
		}
		now := time.Now()
		pending.Range(func(key, value any) bool {
			pc := value.(*pendingCall)
			if now.After(pc.deadline) && pc.answered.CompareAndSwap(false, true) {
				_ = s.send(pc.timeout)
				pending.Delete(key)
			}
			return true
		})
	}
}

func (s *Server) drainInFlight() {
	deadline := time.Now().Add(5 * time.Second)
	for s.inFlight.Load() > 0 && time.Now().Before(deadline) {
		time.Sleep(slotPoll)
	}
}

func (s *Server) send(resp rpcResponse) bool {
	if s.broken.Load() {
		return false
	}
	data, err := json.Marshal(resp)
	if err != nil {
		data = []byte(`{"jsonrpc":"2.0","id":null,"error":{"code":-32603,"message":"response could not be serialized"}}`)
	}
	if len(data) > maxResponseBytes {
		trunc := map[string]any{
			"jsonrpc": "2.0",
			"id":      jsonRawOrNull(resp.ID),
			"result": map[string]any{
				"content": []map[string]any{{
					"type": "text",
					"text": mustJSON(map[string]any{"ok": false, "code": "truncated", "error": "response exceeded body cap", "truncated": true}),
				}},
				"isError":   true,
				"truncated": true,
			},
		}
		data, _ = json.Marshal(trunc)
	}
	s.outMu.Lock()
	defer s.outMu.Unlock()
	if _, err := s.Out.Write(append(data, '\n')); err != nil {
		s.broken.Store(true)
		return false
	}
	return true
}

func (s *Server) handle(req rpcRequest) rpcResponse {
	switch req.Method {
	case "initialize":
		return okResult(req.ID, map[string]any{
			"protocolVersion": "2024-11-05",
			"capabilities":    map[string]any{"tools": map[string]any{}},
			"serverInfo":      map[string]any{"name": "devcouncil", "version": "0.1.0"},
		})
	case "ping":
		return okResult(req.ID, map[string]any{})
	case "tools/list":
		return okResult(req.ID, s.listTools())
	case "tools/call":
		return s.callTool(req)
	default:
		return errResult(req.ID, -32601, "method not found")
	}
}

func (s *Server) listTools() map[string]any {
	specs := s.Registry.Specs()
	tools := make([]map[string]any, 0, len(specs))
	for _, spec := range specs {
		var schema any
		_ = json.Unmarshal(spec.InputSchema, &schema)
		tools = append(tools, map[string]any{
			"name":        spec.Name,
			"description": spec.Description,
			"inputSchema": schema,
			"annotations": spec.Behaviour.Annotations(),
		})
	}
	return map[string]any{"tools": tools}
}

func (s *Server) callTool(req rpcRequest) rpcResponse {
	var params toolsCallParams
	if err := json.Unmarshal(req.Params, &params); err != nil {
		return errResult(req.ID, -32602, "invalid params")
	}
	if params.Arguments == nil {
		params.Arguments = map[string]any{}
	}
	ctx, cancel := context.WithTimeout(context.Background(), callBudget())
	defer cancel()
	payload, err := s.Registry.Call(ctx, params.Name, params.Arguments)
	if err != nil {
		return okResult(req.ID, map[string]any{
			"content": []map[string]any{{"type": "text", "text": mustJSON(map[string]any{"ok": false, "error": err.Error(), "code": "tool_error"})}},
			"isError": true,
		})
	}
	text := mustJSON(payload)
	isError := false
	if m, ok := payload.(map[string]any); ok {
		if okFlag, exists := m["ok"].(bool); exists && !okFlag {
			isError = true
		}
	} else if ep, ok := payload.(devcouncil.ErrorPayload); ok && !ep.OK {
		isError = true
	} else if dr, ok := payload.(devcouncil.DiffResult); ok && !dr.OK {
		isError = true
	}
	return okResult(req.ID, map[string]any{
		"content": []map[string]any{{"type": "text", "text": text}},
		"isError": isError,
	})
}

var errFrameTooLarge = fmt.Errorf("frame exceeds %d bytes", maxFrameBytes)

func readFrame(r *bufio.Reader, max int) ([]byte, error) {
	var buf bytes.Buffer
	for {
		b, err := r.ReadByte()
		if err != nil {
			if err == io.EOF && buf.Len() == 0 {
				return nil, io.EOF
			}
			if err == io.EOF {
				return buf.Bytes(), nil
			}
			return nil, err
		}
		if b == '\n' {
			return buf.Bytes(), nil
		}
		if buf.Len() >= max {
			// Drain to newline so the next frame can be parsed.
			for {
				b, err := r.ReadByte()
				if err != nil || b == '\n' {
					break
				}
			}
			return nil, errFrameTooLarge
		}
		buf.WriteByte(b)
	}
}

func okResult(id json.RawMessage, result any) rpcResponse {
	return rpcResponse{JSONRPC: "2.0", ID: id, Result: result}
}

func errResult(id json.RawMessage, code int, message string) rpcResponse {
	return rpcResponse{JSONRPC: "2.0", ID: id, Error: &rpcError{Code: code, Message: message}}
}

func mustJSON(v any) string {
	b, err := json.Marshal(v)
	if err != nil {
		return `{"ok":false,"error":"marshal failed"}`
	}
	return string(b)
}

func jsonRawOrNull(id json.RawMessage) any {
	if len(id) == 0 {
		return nil
	}
	var v any
	if err := json.Unmarshal(id, &v); err != nil {
		return nil
	}
	return v
}
