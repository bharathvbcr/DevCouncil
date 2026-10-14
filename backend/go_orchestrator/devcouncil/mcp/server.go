package mcp

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"os/signal"
	"sync"
	"sync/atomic"
	"syscall"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
)

const (
	maxFrameBytes     = 4 * 1024 * 1024
	maxInFlight       = 32
	maxBatchMembers   = 64
	defaultCallBudget = 60 * time.Second
	// verifyCallBudget is what devcouncil_verify_task gets unless
	// callBudgetEnv says otherwise: one verification command may legitimately
	// run for verify.CommandTimeout, so the call that hosts it must outlive it.
	// The 60 s default it used to share with every other tool cut a suite short
	// and, because the run was written on the expired context, lost its record.
	verifyCallBudget = verify.CommandTimeout + 2*time.Minute
	verifyToolName   = "devcouncil_verify_task"
	callBudgetEnv    = "DEVCOUNCIL_MCP_CALL_TIMEOUT_MS"
	slotPoll         = 2 * time.Millisecond
	maxResponseBytes = 4 * 1024 * 1024
	drainGrace       = 5 * time.Second
)

// Server is a hand-rolled JSON-RPC MCP stdio server.
type Server struct {
	Registry *devcouncil.Registry
	In       io.Reader
	Out      io.Writer
	Err      io.Writer

	// DefaultProtocol is what an `initialize` that names no supported
	// revision is answered with: "legacy" (the default, the newest handshake
	// revision), "modern" (refuse with the supported list) or one handshake
	// revision. Empty reads DEVCOUNCIL_MCP_PROTOCOL_DEFAULT. See protocol.go.
	DefaultProtocol string

	inFlight atomic.Int32
	broken   atomic.Bool
	outMu    sync.Mutex

	callsMu sync.Mutex
	calls   map[string]*call
}

// call is one request that has been accepted and not yet answered.
type call struct {
	cancel context.CancelFunc
	// answered is claimed by whoever replies first: the handler, or the
	// deadline. Exactly one reply is ever written for a call.
	answered atomic.Bool
	// abandoned marks a call the client cancelled, which is never answered
	// (MCP: a receiver does not respond to a request it was told to abandon).
	abandoned atomic.Bool
}

// rpcRequest is a request that passed validation: it has an id, a version of
// "2.0" and a string method.
type rpcRequest struct {
	ID     json.RawMessage
	Method string
	Params json.RawMessage
}

type rpcResponse struct {
	JSONRPC string `json:"jsonrpc"`
	// ID is never omitted: a nil RawMessage marshals as null, which is what
	// JSON-RPC asks for when the request's id could not be read.
	ID     json.RawMessage `json:"id"`
	Result any             `json:"result,omitempty"`
	Error  *rpcError       `json:"error,omitempty"`
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

// budgetFromEnv reads DEVCOUNCIL_MCP_CALL_TIMEOUT_MS: milliseconds, applied to
// every tool call when set to a positive integer.
func budgetFromEnv() (time.Duration, bool) {
	if raw := os.Getenv(callBudgetEnv); raw != "" {
		var ms int64
		if _, err := fmt.Sscanf(raw, "%d", &ms); err == nil && ms > 0 {
			return time.Duration(ms) * time.Millisecond, true
		}
	}
	return 0, false
}

// callBudgetFor is how long a request may run before the host answers it with a
// timeout and cancels what it started.
func callBudgetFor(method string, params json.RawMessage) time.Duration {
	if d, ok := budgetFromEnv(); ok {
		return d
	}
	if method == "tools/call" {
		var p struct {
			Name string `json:"name"`
		}
		if json.Unmarshal(params, &p) == nil && p.Name == verifyToolName {
			return verifyCallBudget
		}
	}
	return defaultCallBudget
}

// shutdownSignals end a session the way stdin closing does. On Windows Go
// delivers os.Interrupt for Ctrl-C/Ctrl-Break and syscall.SIGTERM for the
// console close, logoff and shutdown events, so the one list covers both.
var shutdownSignals = []os.Signal{os.Interrupt, syscall.SIGTERM}

// SignalError is what Serve returns when a shutdown signal ended the session.
// It is an error rather than nil because the client did not end the session:
// the host was told to stop with calls possibly still owed an answer.
type SignalError struct {
	Signal os.Signal
}

func (e *SignalError) Error() string {
	return fmt.Sprintf("stopped by %v; running calls were cancelled", e.Signal)
}

// frameRead is one readFrame result handed from the reader to the serve loop.
type frameRead struct {
	line []byte
	err  error
}

// Serve runs until stdin closes, the stdout pipe breaks, or the process is sent
// SIGINT or SIGTERM (the Windows console events included). However it stops,
// it cancels every call still running before it waits for them, so what a call
// started (a verification command and its process group) is stopped by the
// time Serve returns, not left behind with PPID 1. A signal ends it with a
// *SignalError; a second signal after the first takes the default action.
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
	signals := make(chan os.Signal, 1)
	signal.Notify(signals, shutdownSignals...)
	defer signal.Stop(signals)

	root, stop := context.WithCancel(context.Background())
	defer stop()
	finish := func(err error) error {
		stop()
		s.drainInFlight()
		return err
	}

	// The read runs on its own goroutine so a signal is not stuck behind a
	// blocking read of stdin. Frames are still dispatched one at a time, in
	// order, by the loop below. quit releases a reader that has a frame in hand
	// once the loop is gone; one blocked in the read itself ends with the
	// process, which is the only reason the loop stops while stdin is open.
	frames := make(chan frameRead)
	quit := make(chan struct{})
	defer close(quit)
	go func() {
		reader := bufio.NewReaderSize(s.In, 64*1024)
		for {
			line, err := readFrame(reader, maxFrameBytes)
			select {
			case frames <- frameRead{line: line, err: err}:
			case <-quit:
				return
			}
			if err != nil && err != errFrameTooLarge {
				return
			}
		}
	}()

	for {
		if s.broken.Load() {
			return finish(io.ErrClosedPipe)
		}
		var line []byte
		var err error
		select {
		case sig := <-signals:
			// Restore the default action now, so a host that hangs while
			// draining can still be stopped by a second signal.
			signal.Stop(signals)
			fmt.Fprintf(s.Err, "devcouncil-mcp: %v received; cancelling running calls\n", sig)
			return finish(&SignalError{Signal: sig})
		case fr := <-frames:
			line, err = fr.line, fr.err
		}
		if err == io.EOF {
			return finish(nil)
		}
		if err != nil {
			fmt.Fprintf(s.Err, "devcouncil-mcp: frame error: %v\n", err)
			if err == errFrameTooLarge {
				// The frame was discarded unread, so its id is unknowable.
				_ = s.send(errResult(nil, -32600, err.Error()+"; the frame was discarded"))
				continue
			}
			return finish(err)
		}
		frame := bytes.TrimSpace(line)
		if len(frame) == 0 {
			continue
		}
		if frame[0] == '[' {
			if !s.startBatch(root, frame) {
				return finish(io.ErrClosedPipe)
			}
			continue
		}
		req, resp := s.decode(frame)
		if resp != nil {
			_ = s.send(*resp)
			continue
		}
		if req != nil && !s.startCall(root, req) {
			return finish(io.ErrClosedPipe)
		}
	}
}

// decode validates one frame and says what to do with it. It returns a
// request to run, or an immediate response, or neither (a notification, or a
// frame nothing is owed for). JSON-RPC 2.0 §4: a frame with an "id" member is a
// request and is owed exactly one response carrying that id, however wrong the
// rest of it is; one without is a notification and is never answered.
func (s *Server) decode(frame []byte) (*rpcRequest, *rpcResponse) {
	if !json.Valid(frame) {
		r := errResult(nil, -32700, "parse error")
		return nil, &r
	}
	if frame[0] != '{' {
		r := errResult(nil, -32600, "a request must be an object or a batch array")
		return nil, &r
	}
	var obj map[string]json.RawMessage
	if err := json.Unmarshal(frame, &obj); err != nil {
		r := errResult(nil, -32600, "a request must be an object or a batch array")
		return nil, &r
	}
	rawID, hasID := obj["id"]
	respond := func(code int, msg string) (*rpcRequest, *rpcResponse) {
		if !hasID {
			return nil, nil
		}
		r := errResult(rawID, code, msg)
		return nil, &r
	}
	if hasID {
		if fault := idFault(rawID); fault != "" {
			return respond(-32600, "a request id must be a string or an integer and must not be null; this one is "+fault)
		}
	}
	_, hasMethod := obj["method"]
	if !hasMethod && (obj["result"] != nil || obj["error"] != nil) {
		return nil, nil // a response to a request we never send
	}
	var version string
	if json.Unmarshal(obj["jsonrpc"], &version) != nil || version != "2.0" {
		return respond(-32600, `jsonrpc must be "2.0"`)
	}
	var method string
	if json.Unmarshal(obj["method"], &method) != nil {
		return respond(-32600, "request has no string 'method'")
	}
	params := obj["params"]

	if !hasID {
		if method == "notifications/cancelled" {
			var p struct {
				RequestID json.RawMessage `json:"requestId"`
			}
			if json.Unmarshal(params, &p) == nil && len(p.RequestID) > 0 {
				s.cancelCall(string(compactID(p.RequestID)))
			}
		}
		// Every other notification, and any request method sent without an id,
		// is dropped: running a tool whose result cannot be returned is work
		// nobody can see.
		return nil, nil
	}
	return &rpcRequest{ID: rawID, Method: method, Params: params}, nil
}

// idFault says why raw is not a legal MCP request id, or "" when it is: a
// string or an integer, never null or a fractional number.
func idFault(raw json.RawMessage) string {
	var v any
	dec := json.NewDecoder(bytes.NewReader(raw))
	dec.UseNumber()
	if dec.Decode(&v) != nil {
		return "unreadable"
	}
	switch id := v.(type) {
	case string:
		return ""
	case json.Number:
		if _, err := id.Int64(); err == nil {
			return ""
		}
		return "a fractional number"
	case nil:
		return "null"
	default:
		return "not a string or an integer"
	}
}

func compactID(raw json.RawMessage) json.RawMessage {
	var b bytes.Buffer
	if json.Compact(&b, raw) != nil {
		return raw
	}
	return b.Bytes()
}

func (s *Server) register(key string, c *call) bool {
	s.callsMu.Lock()
	defer s.callsMu.Unlock()
	if s.calls == nil {
		s.calls = map[string]*call{}
	}
	if _, dup := s.calls[key]; dup {
		return false
	}
	s.calls[key] = c
	return true
}

func (s *Server) release(key string) {
	s.callsMu.Lock()
	delete(s.calls, key)
	s.callsMu.Unlock()
}

// cancelCall stops the in-flight call named by a notifications/cancelled. A
// call that already finished, or never existed, is nothing to cancel.
func (s *Server) cancelCall(key string) {
	s.callsMu.Lock()
	c := s.calls[key]
	s.callsMu.Unlock()
	if c != nil {
		c.abandoned.Store(true)
		c.cancel()
	}
}

// acquireSlot waits for room under maxInFlight, which is the host's
// back-pressure on a client that pipelines without waiting. It reports false if
// the output pipe broke while waiting.
func (s *Server) acquireSlot() bool {
	for s.inFlight.Load() >= maxInFlight {
		if s.broken.Load() {
			return false
		}
		time.Sleep(slotPoll)
	}
	s.inFlight.Add(1)
	return true
}

func timeoutResponse(id json.RawMessage, budget time.Duration) rpcResponse {
	return errResult(id, -32000, fmt.Sprintf("request timed out after %s; set %s (milliseconds) to change the budget", budget, callBudgetEnv))
}

// startCall runs one request in its own goroutine under its own budget. It
// reports false if the output pipe broke.
func (s *Server) startCall(root context.Context, req *rpcRequest) bool {
	key := string(compactID(req.ID))
	budget := callBudgetFor(req.Method, req.Params)
	ctx, cancel := context.WithTimeout(root, budget)
	c := &call{cancel: cancel}
	// A duplicate id is refused, not allowed to replace the call it collides
	// with: two calls under one key would leave the first unreachable by a
	// cancellation and let the second's cleanup erase it.
	if !s.register(key, c) {
		cancel()
		_ = s.send(errResult(req.ID, -32600, "a request with this id is already in flight; ids must be unique until answered"))
		return true
	}
	if !s.acquireSlot() {
		s.release(key)
		cancel()
		return false
	}
	timer := time.AfterFunc(budget, func() {
		if c.answered.CompareAndSwap(false, true) && !c.abandoned.Load() {
			_ = s.send(timeoutResponse(req.ID, budget))
		}
		cancel()
	})
	go func() {
		defer s.inFlight.Add(-1)
		defer s.release(key)
		defer timer.Stop()
		defer cancel()
		resp := s.handle(ctx, *req)
		if c.answered.CompareAndSwap(false, true) && !c.abandoned.Load() {
			_ = s.send(resp)
		}
	}()
	return true
}

// startBatch answers a JSON-RPC batch with one array holding a response per
// request in it (none for a notification, and nothing at all when every member
// was one). Members run in order, sharing one deadline: the longest budget any
// member would have had, so a batch cannot multiply the per-call bound.
func (s *Server) startBatch(root context.Context, frame []byte) bool {
	var items []json.RawMessage
	if err := json.Unmarshal(frame, &items); err != nil {
		_ = s.send(errResult(nil, -32700, "parse error"))
		return true
	}
	if len(items) == 0 {
		_ = s.send(errResult(nil, -32600, "a batch must contain at least one request"))
		return true
	}
	if len(items) > maxBatchMembers {
		_ = s.send(errResult(nil, -32600, fmt.Sprintf("a batch may hold at most %d requests; send fewer", maxBatchMembers)))
		return true
	}
	if !s.acquireSlot() {
		return false
	}
	go func() {
		defer s.inFlight.Add(-1)
		var budget time.Duration
		type member struct {
			req  *rpcRequest
			resp *rpcResponse
		}
		members := make([]member, len(items))
		for i, raw := range items {
			item := bytes.TrimSpace(raw)
			if len(item) > 0 && item[0] == '[' {
				r := errResult(nil, -32600, "a batch member must be a request object; batches do not nest")
				members[i].resp = &r
				continue
			}
			members[i].req, members[i].resp = s.decode(item)
			if members[i].req != nil {
				if b := callBudgetFor(members[i].req.Method, members[i].req.Params); b > budget {
					budget = b
				}
			}
		}
		if budget == 0 {
			budget = defaultCallBudget
		}
		ctx, cancel := context.WithTimeout(root, budget)
		defer cancel()
		var out []rpcResponse
		for _, m := range members {
			switch {
			case m.resp != nil:
				out = append(out, *m.resp)
			case m.req != nil:
				if r := s.runBatchMember(ctx, m.req, budget); r != nil {
					out = append(out, *r)
				}
			}
		}
		if len(out) > 0 {
			s.sendBatch(out)
		}
	}()
	return true
}

func (s *Server) runBatchMember(ctx context.Context, req *rpcRequest, budget time.Duration) *rpcResponse {
	key := string(compactID(req.ID))
	memberCtx, cancel := context.WithCancel(ctx)
	defer cancel()
	c := &call{cancel: cancel}
	if !s.register(key, c) {
		r := errResult(req.ID, -32600, "a request with this id is already in flight; ids must be unique until answered")
		return &r
	}
	defer s.release(key)
	if errors.Is(ctx.Err(), context.DeadlineExceeded) {
		r := timeoutResponse(req.ID, budget)
		return &r
	}
	resp := s.handle(memberCtx, *req)
	if c.abandoned.Load() {
		return nil
	}
	if errors.Is(ctx.Err(), context.DeadlineExceeded) && resp.Error == nil {
		// The shared deadline passed while this member ran; what it returned
		// is a partial answer, so it is reported as the timeout it was.
		r := timeoutResponse(req.ID, budget)
		return &r
	}
	return &resp
}

// drainInFlight waits, bounded, for the calls cancelled at shutdown to finish
// unwinding: their runners kill the process group they started before they
// return.
func (s *Server) drainInFlight() {
	deadline := time.Now().Add(drainGrace)
	for s.inFlight.Load() > 0 && time.Now().Before(deadline) {
		time.Sleep(slotPoll)
	}
}

func (s *Server) send(resp rpcResponse) bool {
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
	return s.write(data)
}

// sendBatch writes a batch answer. One that passes the response cap is refused
// whole: a cut array would silently drop answers its client is waiting for.
func (s *Server) sendBatch(out []rpcResponse) bool {
	data, err := json.Marshal(out)
	if err != nil || len(data) > maxResponseBytes {
		return s.send(errResult(nil, -32603, fmt.Sprintf("this batch's answers passed %d bytes; send fewer requests per batch", maxResponseBytes)))
	}
	return s.write(data)
}

func (s *Server) write(data []byte) bool {
	if s.broken.Load() {
		return false
	}
	s.outMu.Lock()
	defer s.outMu.Unlock()
	if _, err := s.Out.Write(append(data, '\n')); err != nil {
		s.broken.Store(true)
		return false
	}
	return true
}

func (s *Server) handle(ctx context.Context, req rpcRequest) rpcResponse {
	modern, refusal := eraOf(req)
	if refusal != nil {
		return rpcResponse{JSONRPC: "2.0", ID: req.ID, Error: refusal}
	}
	if modern {
		return s.modernHandle(ctx, req)
	}
	return s.legacyHandle(ctx, req)
}

func (s *Server) legacyHandle(ctx context.Context, req rpcRequest) rpcResponse {
	switch req.Method {
	case "initialize":
		return s.handleInitialize(req)
	case "server/discover":
		return s.discoverResult(req)
	case "ping", "notifications/initialized", "notifications/cancelled", "notifications/roots/list_changed":
		// A notification method that arrives with an id is a request, and a
		// request is owed a response; the empty result is devmap's answer too.
		return okResult(req.ID, map[string]any{})
	case "tools/list":
		return okResult(req.ID, s.listTools())
	case "tools/call":
		return s.callTool(ctx, req)
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

func (s *Server) callTool(ctx context.Context, req rpcRequest) rpcResponse {
	var params toolsCallParams
	if err := json.Unmarshal(req.Params, &params); err != nil {
		return errResult(req.ID, -32602, "invalid params")
	}
	if params.Arguments == nil {
		params.Arguments = map[string]any{}
	}
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
