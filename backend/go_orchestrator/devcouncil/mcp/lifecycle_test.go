//go:build unix

package mcp_test

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/mcp"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// liveHost is a Server on pipes with a real store holding one task whose
// verification command starts a background `sleep` and records its pid.
type liveHost struct {
	t       *testing.T
	in      *io.PipeWriter
	out     *syncBuffer
	done    chan error
	pidFile string
	nextID  int
}

func startLiveHost(t *testing.T, command string) *liveHost {
	t.Helper()
	root := t.TempDir()
	client := store.New(testsupport.DCStore(t), filepath.Join(t.TempDir(), "state.sqlite"))
	if _, err := client.ActiveLeases(context.Background()); err != nil {
		t.Fatal(err)
	}
	pidFile := filepath.Join(root, "child.pid")
	scope, _ := json.Marshal(map[string]any{"expected_tests": []string{strings.ReplaceAll(command, "PIDFILE", pidFile)}})
	testsupport.SeedTask(t, client.DB, testsupport.TaskSeed{ID: "TASK-L", Title: "live", Scope: string(scope)})
	pr, pw := io.Pipe()
	h := &liveHost{t: t, in: pw, out: &syncBuffer{}, done: make(chan error, 1), pidFile: pidFile}
	srv := &mcp.Server{Registry: devcouncil.NewRegistry(root, client, nil), In: pr, Out: h.out, Err: io.Discard}
	go func() { h.done <- srv.Serve() }()
	t.Cleanup(func() { _ = pw.Close() })
	return h
}

func (h *liveHost) send(frame string) {
	h.t.Helper()
	if _, err := io.WriteString(h.in, frame+"\n"); err != nil {
		h.t.Fatal(err)
	}
}

func (h *liveHost) request(method, params string) int {
	h.nextID++
	h.send(fmt.Sprintf(`{"jsonrpc":"2.0","id":%d,"method":%q,"params":%s}`, h.nextID, method, params))
	return h.nextID
}

// replyFor waits for the response with the given id.
func (h *liveHost) replyFor(id int, within time.Duration) (string, bool) {
	deadline := time.Now().Add(within)
	for time.Now().Before(deadline) {
		for _, l := range h.out.lines() {
			var r struct {
				ID json.RawMessage `json:"id"`
			}
			if json.Unmarshal([]byte(l), &r) == nil && string(r.ID) == strconv.Itoa(id) {
				return l, true
			}
		}
		time.Sleep(10 * time.Millisecond)
	}
	return "", false
}

// startVerify checks the task out and starts a verify that will run until
// something stops it. It returns the verify request id and the child's pid.
func (h *liveHost) startVerify() (int, int) {
	h.t.Helper()
	co := h.request("tools/call", `{"name":"devcouncil_checkout_task","arguments":{"task_id":"TASK-L","client_id":"c1"}}`)
	line, ok := h.replyFor(co, 10*time.Second)
	if !ok {
		h.t.Fatal("no checkout reply")
	}
	var env struct {
		Result struct {
			Content []struct {
				Text string `json:"text"`
			} `json:"content"`
		} `json:"result"`
	}
	if err := json.Unmarshal([]byte(line), &env); err != nil || len(env.Result.Content) == 0 {
		h.t.Fatalf("checkout reply %s: %v", line, err)
	}
	var payload struct {
		Token string `json:"lease_token"`
	}
	if err := json.Unmarshal([]byte(env.Result.Content[0].Text), &payload); err != nil || payload.Token == "" {
		h.t.Fatalf("no lease token in %s", env.Result.Content[0].Text)
	}
	v := h.request("tools/call", fmt.Sprintf(`{"name":"devcouncil_verify_task","arguments":{"task_id":"TASK-L","lease_token":%q}}`, payload.Token))
	return v, h.waitPid()
}

func (h *liveHost) waitPid() int {
	h.t.Helper()
	deadline := time.Now().Add(15 * time.Second)
	for time.Now().Before(deadline) {
		if b, err := os.ReadFile(h.pidFile); err == nil {
			if pid, err := strconv.Atoi(strings.TrimSpace(string(b))); err == nil {
				return pid
			}
		}
		time.Sleep(20 * time.Millisecond)
	}
	h.t.Fatal("the verification command never started")
	return 0
}

func waitGone(t *testing.T, pid int, what string) {
	t.Helper()
	deadline := time.Now().Add(10 * time.Second)
	for time.Now().Before(deadline) {
		if err := syscall.Kill(pid, 0); errors.Is(err, syscall.ESRCH) {
			return
		}
		time.Sleep(20 * time.Millisecond)
	}
	_ = syscall.Kill(pid, syscall.SIGKILL)
	t.Fatalf("pid %d survived %s", pid, what)
}

const sleeper = "sleep 120 & echo $! > PIDFILE; wait"

func TestNotificationsCancelledStopsTheMatchingCall(t *testing.T) {
	h := startLiveHost(t, sleeper)
	verifyID, pid := h.startVerify()

	h.send(fmt.Sprintf(`{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":%d}}`, verifyID))
	waitGone(t, pid, "notifications/cancelled")

	// A cancelled request is not answered; the host stays usable.
	ping := h.request("ping", `{}`)
	if _, ok := h.replyFor(ping, 5*time.Second); !ok {
		t.Fatal("the host stopped answering after a cancellation")
	}
	if line, answered := h.replyFor(verifyID, 500*time.Millisecond); answered {
		t.Errorf("a cancelled request was answered: %s", line)
	}
}

func TestADuplicateInFlightIDIsRefusedAndTheOriginalSurvives(t *testing.T) {
	h := startLiveHost(t, sleeper)
	verifyID, pid := h.startVerify()

	h.send(fmt.Sprintf(`{"jsonrpc":"2.0","id":%d,"method":"ping"}`, verifyID))
	deadline := time.Now().Add(5 * time.Second)
	var refusal string
	for time.Now().Before(deadline) && refusal == "" {
		for _, l := range h.out.lines() {
			if strings.Contains(l, fmt.Sprintf(`"id":%d`, verifyID)) && strings.Contains(l, "-32600") {
				refusal = l
			}
		}
		time.Sleep(10 * time.Millisecond)
	}
	if refusal == "" {
		t.Fatal("a duplicate in-flight id was not refused")
	}
	if err := syscall.Kill(pid, 0); err != nil {
		t.Fatalf("the original call was disturbed by the duplicate: %v", err)
	}
	h.send(fmt.Sprintf(`{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":%d}}`, verifyID))
	waitGone(t, pid, "cleanup cancel")
}

func TestEOFCancelsRunningCallsAndTheirProcessGroupsBeforeServeReturns(t *testing.T) {
	h := startLiveHost(t, sleeper)
	_, pid := h.startVerify()

	_ = h.in.Close()
	select {
	case <-h.done:
	case <-time.After(15 * time.Second):
		t.Fatal("Serve did not return after stdin closed")
	}
	// Serve has returned: nothing it started may still be running.
	if err := syscall.Kill(pid, 0); !errors.Is(err, syscall.ESRCH) {
		_ = syscall.Kill(pid, syscall.SIGKILL)
		t.Fatalf("pid %d outlived Serve (kill -0: %v)", pid, err)
	}
}

// The budget of a call comes from DEVCOUNCIL_MCP_CALL_TIMEOUT_MS, and the
// timeout reply names it.
func TestTheTimeoutReplyNamesTheBudgetKnob(t *testing.T) {
	t.Setenv("DEVCOUNCIL_MCP_CALL_TIMEOUT_MS", "300")
	h := startLiveHost(t, sleeper)
	verifyID, pid := h.startVerify()
	line, ok := h.replyFor(verifyID, 10*time.Second)
	if !ok {
		t.Fatal("no timeout reply")
	}
	if !strings.Contains(line, "DEVCOUNCIL_MCP_CALL_TIMEOUT_MS") {
		t.Errorf("timeout reply does not name the knob: %s", line)
	}
	waitGone(t, pid, "the call budget")
	_ = bufio.NewReader
}
