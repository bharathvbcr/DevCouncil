package devmap

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
)

type cliStatusOnlyKey struct{}

func withCLIStatus(ctx context.Context) context.Context {
	return context.WithValue(ctx, cliStatusOnlyKey{}, true)
}

func isCLIStatusOnly(ctx context.Context) bool {
	v, _ := ctx.Value(cliStatusOnlyKey{}).(bool)
	return v
}

var (
	errWarmUnsupported = errors.New("devmap binary does not speak MCP over stdio")
	errWarmFallbackCLI = errors.New("fallback to CLI invocation")
)

const (
	warmHandshakeTimeout = 10 * time.Second
	warmMinMaxFrameBytes = 16 << 20
	mcpMaxBudget         = 100_000
	mcpMaxTargetBytes    = 4096
)

// EnableWarmMCP switches this client onto a persistent `devmap mcp` stdio
// session (`P0.3`). Call Close when the client is no longer needed.
//
// The process outlives any single call, so with no owner it lives until Close.
// Prefer EnableWarmMCPContext, which ties it to a caller's lifetime.
func (c *Client) EnableWarmMCP() {
	c.EnableWarmMCPContext(context.Background())
}

// EnableWarmMCPContext is EnableWarmMCP with an owner: the warm `devmap mcp`
// process is a child of ctx, and cancelling ctx closes the session and kills
// the process group. Only the first owner is kept; a client has one lifetime.
func (c *Client) EnableWarmMCPContext(ctx context.Context) {
	if ctx == nil {
		ctx = context.Background()
	}
	c.warmMu.Lock()
	c.warmEnabled = true
	register := c.ownerCtx == nil
	if register {
		c.ownerCtx = ctx
	}
	c.warmMu.Unlock()
	if register && ctx.Done() != nil {
		context.AfterFunc(ctx, func() { _ = c.Close() })
	}
}

// WarmSession returns a client backed by a warm `devmap mcp` stdio session for
// multi-query bursts (such as 25-file `graph_context` fan-out).
//
// If c already has warm MCP enabled, the returned handle shares c's session and
// Close on the returned handle is a no-op (c owns shutdown). Otherwise, the
// returned handle owns a scoped `devmap mcp` process that Close terminates,
// while sharing c's cached capability probe verdict.
func (c *Client) WarmSession() *Client {
	c.warmMu.Lock()
	shared := c.warmEnabled
	c.warmMu.Unlock()

	c.warmMu.Lock()
	owner := c.ownerCtx
	c.warmMu.Unlock()

	child := &Client{
		Binary:       c.Binary,
		Root:         c.Root,
		Timeout:      c.Timeout,
		Budget:       c.Budget,
		maxOutput:    c.maxOutput,
		maxStderr:    c.maxStderr,
		probeTimeout: c.probeTimeout,
		warmEnabled:  true,
		warmShared:   shared,
		probeParent:  c,
		ownerCtx:     owner,
	}
	if !shared && owner != nil && owner.Done() != nil {
		context.AfterFunc(owner, func() { _ = child.Close() })
	}
	return child
}

// Close terminates any warm `devmap mcp` subprocess owned by this client.
func (c *Client) Close() error {
	if c == nil || c.warmShared {
		return nil
	}
	c.warmMu.Lock()
	sess := c.warm
	c.warm = nil
	c.warmMu.Unlock()
	if sess == nil {
		return nil
	}
	return sess.close()
}

type rpcErrorObj struct {
	Code    int    `json:"code"`
	Message string `json:"message"`
}

type rpcResponse struct {
	result json.RawMessage
	rpcErr *rpcErrorObj
	err    error
}

type mcpSession struct {
	rootCanon  string
	cmd        *exec.Cmd
	procCancel context.CancelFunc
	stdin      io.WriteCloser
	stderr     *proc.CappedBuffer
	maxFrame   int

	writeMu sync.Mutex
	nextID  int64

	mu       sync.Mutex
	pending  map[int64]chan rpcResponse
	closed   bool
	closeErr error
	done     chan struct{}
	waitDone chan struct{}
}

func startMCPSession(ctx, owner context.Context, binary, root string, maxOut, maxErr int) (*mcpSession, error) {
	if binary == "" {
		return nil, errors.New("no devmap binary configured")
	}
	if root == "" {
		return nil, errors.New("no repository root configured")
	}
	if maxOut <= 0 {
		maxOut = defaultMaxOutput
	}
	if maxErr <= 0 {
		maxErr = defaultMaxStderr
	}
	maxFrame := max(maxOut, warmMinMaxFrameBytes)

	if owner == nil {
		owner = context.Background()
	}
	procCtx, procCancel := context.WithCancel(owner)
	cmd := exec.CommandContext(procCtx, binary, "--root", root, "mcp")
	proc.ConfigureGroup(cmd)
	cmd.Dir = root
	cmd.WaitDelay = 500 * time.Millisecond

	stdin, err := cmd.StdinPipe()
	if err != nil {
		procCancel()
		return nil, err
	}
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		_ = stdin.Close()
		procCancel()
		return nil, err
	}
	stderr := &proc.CappedBuffer{Limit: maxErr}
	cmd.Stderr = stderr

	startCtx, startCancel := context.WithTimeout(ctx, warmHandshakeTimeout)
	startErr, timedOut := proc.RunBounded(startCtx, cmd.Start)
	startCancel()
	if timedOut {
		_ = stdin.Close()
		procCancel()
		return nil, fmt.Errorf("devmap mcp failed to start within %s", warmHandshakeTimeout)
	}
	if startErr != nil {
		_ = stdin.Close()
		procCancel()
		return nil, startErr
	}

	sess := &mcpSession{
		rootCanon:  canonicalRepoRoot(root),
		cmd:        cmd,
		procCancel: procCancel,
		stdin:      stdin,
		stderr:     stderr,
		maxFrame:   maxFrame,
		nextID:     1,
		pending:    make(map[int64]chan rpcResponse),
		done:       make(chan struct{}),
		waitDone:   make(chan struct{}),
	}

	// One goroutine owns the process's end of life, in this order: read stdout
	// to EOF, reap the child, then fail what is pending. Failing on the bare EOF
	// reported "EOF" before the exit status and stderr were knowable.
	go func() {
		readErr := sess.readLoop(stdout)
		if readErr != nil && !errors.Is(readErr, io.EOF) {
			// A protocol failure while the child may still be running: tell the
			// callers now, then stop the child so Wait cannot block forever.
			sess.failAll(readErr)
			if cmd.Process != nil {
				proc.KillGroupWithEscalation(cmd.Process.Pid, 500*time.Millisecond)
			}
		}
		waitErr := cmd.Wait()
		sess.failAll(sess.deathError(waitErr))
		close(sess.waitDone)
	}()

	hsCtx, hsCancel := context.WithTimeout(ctx, warmHandshakeTimeout)
	defer hsCancel()
	if err := sess.handshake(hsCtx); err != nil {
		_ = sess.close()
		if errors.Is(ctx.Err(), context.Canceled) || errors.Is(ctx.Err(), context.DeadlineExceeded) {
			return nil, ctx.Err()
		}
		if tail := sess.stderrTail(); tail != "" {
			return nil, fmt.Errorf("%w: %v (stderr: %s)", errWarmUnsupported, err, tail)
		}
		return nil, fmt.Errorf("%w: %v", errWarmUnsupported, err)
	}
	return sess, nil
}

func (s *mcpSession) handshake(ctx context.Context) error {
	initParams := map[string]any{
		"protocolVersion": "2025-11-25",
		"capabilities":    map[string]any{},
		"clientInfo": map[string]any{
			"name":    "devcouncil-go",
			"version": "0.1.0",
		},
	}
	res, err := s.roundTrip(ctx, "initialize", initParams)
	if err != nil {
		return err
	}
	var parsed struct {
		ProtocolVersion string `json:"protocolVersion"`
	}
	if err := json.Unmarshal(res, &parsed); err != nil || parsed.ProtocolVersion == "" {
		return errors.New("invalid MCP initialize response")
	}
	return s.sendNotification("notifications/initialized", map[string]any{})
}

func (s *mcpSession) sendNotification(method string, params any) error {
	frame, err := json.Marshal(map[string]any{
		"jsonrpc": "2.0",
		"method":  method,
		"params":  params,
	})
	if err != nil {
		return err
	}
	frame = append(frame, '\n')
	s.writeMu.Lock()
	defer s.writeMu.Unlock()
	_, err = s.stdin.Write(frame)
	return err
}

func (s *mcpSession) roundTrip(ctx context.Context, method string, params any) (json.RawMessage, error) {
	id := atomic.AddInt64(&s.nextID, 1)
	ch := make(chan rpcResponse, 1)

	s.mu.Lock()
	if s.closed {
		err := s.closeErr
		s.mu.Unlock()
		if err == nil {
			err = io.EOF
		}
		return nil, err
	}
	s.pending[id] = ch
	s.mu.Unlock()

	frame, err := json.Marshal(map[string]any{
		"jsonrpc": "2.0",
		"id":      id,
		"method":  method,
		"params":  params,
	})
	if err != nil {
		s.removePending(id)
		return nil, err
	}
	frame = append(frame, '\n')

	s.writeMu.Lock()
	_, writeErr := s.stdin.Write(frame)
	s.writeMu.Unlock()
	if writeErr != nil {
		s.removePending(id)
		return nil, writeErr
	}

	select {
	case resp := <-ch:
		if resp.err != nil {
			return nil, resp.err
		}
		if resp.rpcErr != nil {
			return nil, fmt.Errorf("mcp error %d: %s", resp.rpcErr.Code, resp.rpcErr.Message)
		}
		return resp.result, nil
	case <-s.done:
		s.removePending(id)
		s.mu.Lock()
		err := s.closeErr
		s.mu.Unlock()
		if err == nil {
			err = io.EOF
		}
		return nil, err
	case <-ctx.Done():
		s.removePending(id)
		_ = s.sendNotification("notifications/cancelled", map[string]any{
			"requestId": id,
		})
		return nil, ctx.Err()
	}
}

func (s *mcpSession) removePending(id int64) {
	s.mu.Lock()
	delete(s.pending, id)
	s.mu.Unlock()
}

func (s *mcpSession) failAll(err error) {
	s.mu.Lock()
	if s.closed {
		s.mu.Unlock()
		return
	}
	s.closed = true
	if err == nil {
		err = io.EOF
	}
	s.closeErr = err
	pending := s.pending
	s.pending = make(map[int64]chan rpcResponse)
	close(s.done)
	s.mu.Unlock()

	for _, ch := range pending {
		ch <- rpcResponse{err: err}
	}
}

func (s *mcpSession) isAlive() bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	return !s.closed
}

// readLoop dispatches frames until stdout ends and returns why it ended
// (io.EOF for a plain close). It never fails the session itself for an end of
// stream: the exit status is not known yet.
func (s *mcpSession) readLoop(r io.Reader) error {
	br := bufio.NewReaderSize(r, 64<<10)
	for {
		line, overflowed, err := readBoundedLine(br, s.maxFrame)
		if overflowed {
			return errCapped
		}
		if len(line) > 0 {
			s.dispatchLine(line)
		}
		if err != nil {
			return err
		}
	}
}

// warmStderrTail is how much of the child's stderr an error carries.
const warmStderrTail = 2 << 10

// stderrTail is the last bytes the child wrote to stderr, flattened to one
// printable line, for inclusion in an error. Empty when it wrote nothing.
func (s *mcpSession) stderrTail() string {
	if s.stderr == nil {
		return ""
	}
	raw := s.stderr.Bytes()
	if len(raw) > warmStderrTail {
		raw = raw[len(raw)-warmStderrTail:]
	}
	text := strings.Map(func(r rune) rune {
		if r == '\n' || r == '\t' {
			return ' '
		}
		if r < 0x20 || r == 0x7f {
			return -1
		}
		return r
	}, string(raw))
	return strings.TrimSpace(text)
}

// deathError is what a session that ended on its own reports: the exit status
// and the stderr tail, wrapping io.EOF when the child exited cleanly.
func (s *mcpSession) deathError(waitErr error) error {
	cause := waitErr
	if cause == nil {
		cause = io.EOF
	}
	if tail := s.stderrTail(); tail != "" {
		return fmt.Errorf("devmap mcp ended: %w (stderr: %s)", cause, tail)
	}
	return fmt.Errorf("devmap mcp ended: %w", cause)
}

func readBoundedLine(br *bufio.Reader, limit int) ([]byte, bool, error) {
	var buf []byte
	for {
		frag, err := br.ReadSlice('\n')
		if len(buf)+len(frag) > limit {
			// Drain until newline or EOF without retaining more bytes.
			for errors.Is(err, bufio.ErrBufferFull) {
				_, err = br.ReadSlice('\n')
			}
			return nil, true, err
		}
		if err == nil {
			if len(buf) == 0 {
				return bytes.TrimSpace(frag), false, nil
			}
			buf = append(buf, frag...)
			return bytes.TrimSpace(buf), false, nil
		}
		if errors.Is(err, bufio.ErrBufferFull) {
			buf = append(buf, frag...)
			continue
		}
		buf = append(buf, frag...)
		return bytes.TrimSpace(buf), false, err
	}
}

func (s *mcpSession) dispatchLine(line []byte) {
	var raw map[string]json.RawMessage
	if err := json.Unmarshal(line, &raw); err != nil {
		// Non-JSON output on stdout violates MCP stdio framing; fail closed.
		s.failAll(fmt.Errorf("unparseable MCP frame: %w", err))
		return
	}

	if methodRaw, hasMethod := raw["method"]; hasMethod {
		if _, hasResult := raw["result"]; !hasResult {
			if _, hasErr := raw["error"]; !hasErr {
				// Server-initiated request or notification.
				if idRaw, hasID := raw["id"]; hasID && len(idRaw) > 0 && string(idRaw) != "null" {
					var method string
					_ = json.Unmarshal(methodRaw, &method)
					if method == "roots/list" {
						s.replyServerRequest(idRaw, map[string]any{"roots": []any{}})
					}
				}
				return
			}
		}
	}

	idRaw, ok := raw["id"]
	if !ok {
		return
	}
	var id int64
	if err := json.Unmarshal(idRaw, &id); err != nil {
		return
	}

	var resp rpcResponse
	if errRaw, hasErr := raw["error"]; hasErr && string(errRaw) != "null" {
		var rpcErr rpcErrorObj
		if err := json.Unmarshal(errRaw, &rpcErr); err != nil {
			resp.err = fmt.Errorf("malformed MCP error object: %w", err)
		} else {
			resp.rpcErr = &rpcErr
		}
	} else {
		resp.result = raw["result"]
	}

	s.mu.Lock()
	ch, waiting := s.pending[id]
	if waiting {
		delete(s.pending, id)
	}
	s.mu.Unlock()
	if waiting {
		ch <- resp
	}
}

func (s *mcpSession) replyServerRequest(id json.RawMessage, result any) {
	frame, err := json.Marshal(map[string]any{
		"jsonrpc": "2.0",
		"id":      id,
		"result":  result,
	})
	if err != nil {
		return
	}
	frame = append(frame, '\n')
	s.writeMu.Lock()
	_, _ = s.stdin.Write(frame)
	s.writeMu.Unlock()
}

func (s *mcpSession) close() error {
	s.writeMu.Lock()
	_ = s.stdin.Close()
	s.writeMu.Unlock()

	select {
	case <-s.waitDone:
		s.procCancel()
		return nil
	case <-time.After(250 * time.Millisecond):
	}

	if s.cmd != nil && s.cmd.Process != nil {
		proc.KillGroupWithEscalation(s.cmd.Process.Pid, 500*time.Millisecond)
	}
	s.procCancel()
	select {
	case <-s.waitDone:
	case <-time.After(time.Second):
	}
	return nil
}

func canonicalRepoRoot(p string) string {
	if p == "" {
		return ""
	}
	abs, err := filepath.Abs(p)
	if err != nil {
		abs = filepath.Clean(p)
	}
	if resolved, err := filepath.EvalSymlinks(abs); err == nil {
		return filepath.Clean(resolved)
	}
	return filepath.Clean(abs)
}

func (c *Client) decodeWarmMCP(ctx context.Context, into any, timeout time.Duration, args ...string) (bool, error) {
	if c.warmShared && c.probeParent != nil {
		return c.probeParent.decodeWarmMCP(ctx, into, timeout, args...)
	}

	c.warmMu.Lock()
	enabled := c.warmEnabled && !c.warmDisabled
	c.warmMu.Unlock()
	if !enabled || len(args) == 0 {
		return false, nil
	}

	toolName, toolArgs, ok := c.mapWarmToolArgs(ctx, args)
	if !ok {
		return false, nil
	}

	if timeout <= 0 {
		timeout = 30 * time.Second
	}
	callCtx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()

	for attempt := range 2 {
		sess, err := c.getOrStartWarmSession(callCtx)
		if err != nil {
			if errors.Is(err, errWarmUnsupported) {
				c.warmMu.Lock()
				c.warmDisabled = true
				c.warmMu.Unlock()
				return false, nil
			}
			return true, err
		}

		retry, callErr := c.callWarmTool(callCtx, sess, args[0], toolName, toolArgs, into)
		if callErr == nil {
			return true, nil
		}
		if errors.Is(callErr, errWarmFallbackCLI) {
			return false, nil
		}
		if retry && attempt == 0 {
			c.dropWarmSession(sess)
			continue
		}
		// The call's own deadline expired (the caller's context is intact) and
		// the server never answered: it is hung, not slow. Kill it, so the
		// next call starts a fresh process instead of queueing behind it.
		if ctx.Err() == nil && errors.Is(callCtx.Err(), context.DeadlineExceeded) {
			c.dropWarmSession(sess)
		}
		return true, callErr
	}
	return false, nil
}

func (c *Client) getOrStartWarmSession(ctx context.Context) (*mcpSession, error) {
	c.warmMu.Lock()
	if c.warmDisabled {
		c.warmMu.Unlock()
		return nil, errWarmUnsupported
	}
	if c.warm != nil && c.warm.isAlive() {
		sess := c.warm
		c.warmMu.Unlock()
		return sess, nil
	}
	old := c.warm
	c.warm = nil
	c.warmMu.Unlock()

	if old != nil {
		_ = old.close()
	}

	// Build and start under warmMu to serialize concurrent first-call callers onto
	// a single `devmap mcp` subprocess.
	c.warmMu.Lock()
	defer c.warmMu.Unlock()
	if c.warmDisabled {
		return nil, errWarmUnsupported
	}
	if c.warm != nil && c.warm.isAlive() {
		return c.warm, nil
	}
	sess, err := startMCPSession(ctx, c.ownerCtx, c.Binary, c.Root, c.maxOutput, c.maxStderr)
	if err != nil {
		return nil, err
	}
	c.warm = sess
	return sess, nil
}

func (c *Client) dropWarmSession(target *mcpSession) {
	c.warmMu.Lock()
	var toClose *mcpSession
	if c.warm == target {
		toClose = c.warm
		c.warm = nil
	}
	c.warmMu.Unlock()
	if toClose != nil {
		_ = toClose.close()
	}
}

func (c *Client) mapWarmToolArgs(ctx context.Context, args []string) (string, map[string]any, bool) {
	switch args[0] {
	case "status":
		if len(args) != 1 || isCLIStatusOnly(ctx) {
			return "", nil, false
		}
		return "devmap_status", map[string]any{
			"repo_path": c.Root,
		}, true

	case "search":
		// Expected CLI shape: ["search", "--budget", "<N>", "--", "<query>"]
		if len(args) != 5 || args[1] != "--budget" || args[3] != "--" {
			return "", nil, false
		}
		budget, err := strconv.Atoi(args[2])
		if err != nil || budget < 1 || budget > mcpMaxBudget || len(args[4]) > mcpMaxTargetBytes {
			return "", nil, false
		}
		return "devmap_search", map[string]any{
			"repo_path": c.Root,
			"query":     args[4],
			"budget":    budget,
		}, true

	case "deps":
		// Expected CLI shape: ["deps", "--budget", "<N>", "--", "<target>"]
		if len(args) != 5 || args[1] != "--budget" || args[3] != "--" {
			return "", nil, false
		}
		budget, err := strconv.Atoi(args[2])
		if err != nil || budget < 1 || budget > mcpMaxBudget || len(args[4]) > mcpMaxTargetBytes {
			return "", nil, false
		}
		return "devmap_dependencies", map[string]any{
			"repo_path": c.Root,
			"target":    args[4],
			"budget":    budget,
		}, true

	case "dead":
		// Expected CLI shape: ["dead", "--budget", "<N>"]
		if len(args) != 3 || args[1] != "--budget" {
			return "", nil, false
		}
		budget, err := strconv.Atoi(args[2])
		if err != nil || budget < 1 || budget > mcpMaxBudget {
			return "", nil, false
		}
		return "devmap_dead_symbols", map[string]any{
			"repo_path": c.Root,
			"budget":    budget,
		}, true

	default:
		return "", nil, false
	}
}

func (c *Client) callWarmTool(
	ctx context.Context,
	sess *mcpSession,
	cliCmd string,
	toolName string,
	toolArgs map[string]any,
	into any,
) (retry bool, err error) {
	rawResult, err := sess.roundTrip(ctx, "tools/call", map[string]any{
		"name":      toolName,
		"arguments": toolArgs,
	})
	if err != nil {
		if errors.Is(err, errCapped) {
			return false, fmt.Errorf("devmap %s failed: %w", cliCmd, errCapped)
		}
		if ctx.Err() != nil {
			if tail := sess.stderrTail(); tail != "" {
				return false, fmt.Errorf("devmap %s timed out or cancelled: %w (stderr: %s)", cliCmd, ctx.Err(), tail)
			}
			return false, fmt.Errorf("devmap %s timed out or cancelled: %w", cliCmd, ctx.Err())
		}
		// If the child died unexpectedly between calls, allow one clean restart.
		return !sess.isAlive(), fmt.Errorf("devmap %s failed over warm MCP: %w", cliCmd, err)
	}

	outLimit := c.maxOutput
	if outLimit <= 0 {
		outLimit = defaultMaxOutput
	}
	if len(rawResult) > outLimit {
		return false, fmt.Errorf("devmap %s failed: %w", cliCmd, errCapped)
	}

	var callRes struct {
		IsError           bool            `json:"isError"`
		StructuredContent json.RawMessage `json:"structuredContent"`
		Content           []struct {
			Type string `json:"type"`
			Text string `json:"text"`
		} `json:"content"`
	}
	if err := json.Unmarshal(rawResult, &callRes); err != nil {
		return false, fmt.Errorf("devmap %s returned unparseable MCP tool envelope: %w", cliCmd, err)
	}

	if callRes.IsError {
		var parts []string
		for _, item := range callRes.Content {
			if strings.TrimSpace(item.Text) != "" {
				parts = append(parts, strings.TrimSpace(item.Text))
			}
		}
		msg := strings.Join(parts, "; ")
		if msg == "" {
			msg = "unknown MCP tool error"
		}
		if strings.Contains(msg, "binary_replaced:") {
			return true, fmt.Errorf("devmap %s detected binary replacement: %s", cliCmd, msg)
		}
		if cliCmd == "status" && strings.Contains(msg, "no DevMap store found") {
			return false, errWarmFallbackCLI
		}
		return false, fmt.Errorf("devmap %s failed: %s", cliCmd, msg)
	}

	if len(callRes.StructuredContent) == 0 {
		return false, fmt.Errorf("devmap %s returned MCP result without structuredContent", cliCmd)
	}
	if len(callRes.StructuredContent) > outLimit {
		return false, fmt.Errorf("devmap %s failed: %w", cliCmd, errCapped)
	}

	var attribution struct {
		Repository struct {
			Root  string `json:"root"`
			Store string `json:"store"`
		} `json:"repository"`
	}
	if err := json.Unmarshal(callRes.StructuredContent, &attribution); err != nil {
		return false, fmt.Errorf("devmap %s returned unparseable repository attribution: %w", cliCmd, err)
	}
	if attribution.Repository.Root == "" {
		return false, fmt.Errorf("devmap %s returned empty repository.root attribution", cliCmd)
	}
	gotRoot := canonicalRepoRoot(attribution.Repository.Root)
	if gotRoot != sess.rootCanon {
		return false, fmt.Errorf(
			"devmap %s repository.root mismatch: server answered for %q, expected %q",
			cliCmd, gotRoot, sess.rootCanon,
		)
	}

	if err := json.Unmarshal(bytes.TrimSpace(callRes.StructuredContent), into); err != nil {
		return false, fmt.Errorf("devmap %s returned unparseable output: %w", cliCmd, err)
	}
	if st, ok := into.(*Status); ok && st.DBPath == "" && attribution.Repository.Store != "" {
		st.DBPath = attribution.Repository.Store
	}
	return false, nil
}
