package lappi

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"net"
	"os"
	"path/filepath"
	"strings"
	"sync/atomic"
	"testing"
	"time"
)

// shortDir returns a directory whose socket paths fit darwin's 104-byte
// sun_path; t.TempDir() under /var/folders does not.
func shortDir(t *testing.T) string {
	t.Helper()
	dir, err := os.MkdirTemp("", "qd")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := os.RemoveAll(dir); err != nil {
			t.Errorf("cleanup %s: %v", dir, err)
		}
	})
	return dir
}

type fakeAgent struct {
	path    string
	accepts atomic.Int64
	lines   chan []byte
}

// startAgent listens on a fresh socket and runs handle for each connection,
// after reading the request line (which it publishes on lines).
func startAgent(t *testing.T, handle func(conn net.Conn)) *fakeAgent {
	t.Helper()
	path := filepath.Join(shortDir(t), "a.sock")
	if len(path) >= maxSocketPathBytes {
		t.Fatalf("socket path %q is %d bytes, over the darwin limit", path, len(path))
	}
	l, err := net.Listen("unix", path)
	if err != nil {
		t.Fatal(err)
	}
	agent := &fakeAgent{path: path, lines: make(chan []byte, 16)}
	done := make(chan struct{})
	t.Cleanup(func() {
		close(done)
		if err := l.Close(); err != nil {
			t.Errorf("close listener: %v", err)
		}
	})
	go func() {
		for {
			conn, err := l.Accept()
			if err != nil {
				return
			}
			agent.accepts.Add(1)
			go func() {
				defer conn.Close()
				line, err := bufio.NewReader(conn).ReadBytes('\n')
				if err == nil {
					agent.lines <- line
				}
				select {
				case <-done:
				default:
					handle(conn)
				}
			}()
		}
	}()
	return agent
}

func reply(s string) func(net.Conn) {
	return func(conn net.Conn) { _, _ = conn.Write([]byte(s + "\n")) }
}

func testRequest() Request {
	return NewDefectClassRequest([]byte("file: a.go\n\n@@ -1 +1 @@\n-a\n+b"), strings.Repeat("ab", 16))
}

func askWith(t *testing.T, path string, timeout time.Duration) Result {
	t.Helper()
	c, err := NewClient(path, timeout)
	if err != nil {
		t.Fatal(err)
	}
	return c.Ask(context.Background(), testRequest())
}

const answered = `{"status":"ok","schema_version":1,"backend":"qd-metal/qwen3.5-2b-base/tessl","degraded":false,"slots":{` +
	`"defect_class":{"value":"logic","conformal_set":["logic"],"score":0.91,"noul":false,"degraded":false},` +
	`"defect_span":{"value":{"start_line":4,"end_line":4},"conformal_set":null,"score":0.6,"noul":false,"degraded":false}}}`

const abstained = `{"status":"ok","schema_version":1,"backend":"qd-metal/x","degraded":false,"slots":{` +
	`"defect_class":{"value":null,"conformal_set":null,"score":0.01,"noul":true,"degraded":false},` +
	`"defect_span":{"value":null,"conformal_set":null,"score":0.0,"noul":true,"degraded":false}}}`

// degradedAnswer is the reference backend: values present, but not a model's.
const degradedAnswer = `{"status":"ok","schema_version":1,"backend":"reference-deterministic-v1","degraded":true,"slots":{` +
	`"defect_class":{"value":"stub","conformal_set":["stub"],"score":0.7,"noul":false,"degraded":true},` +
	`"defect_span":{"value":{"start_line":3,"end_line":3},"conformal_set":null,"score":0.6,"noul":false,"degraded":true}}}`

const refused = `{"status":"refused","schema_version":1,"refusal":{"kind":"calibration_entry_missing","slot":"defect_span","slot_type":"span","rows":0},"message":"no calibration entry for file: secret/path.go"}`

const backendError = `{"status":"error","schema_version":1,"error":{"kind":"deadline_exceeded","limit_ms":30000},"message":"request exceeded the 30000 ms deadline"}`

// TestEveryOutcomeIsOneOfSix maps a reply of each kind to its reading.
func TestEveryOutcomeIsOneOfSix(t *testing.T) {
	cases := []struct {
		name    string
		reply   string
		reading Reading
		kind    string
	}{
		{"answered", answered, ModelAnswered, ""},
		{"abstained", abstained, ModelAbstained, ""},
		{"ensemble_backend", strings.Replace(answered, `"backend":"qd-metal/qwen3.5-2b-base/tessl"`,
			`"backend":"ensemble/2[qd-metal/qwen3.5-2b-base/tessl,qd-metal/qwen3.5-2b-base/tessl]"`, 1), ModelAnswered, ""},
		{"backend_with_space", strings.Replace(answered, `"backend":"qd-metal/qwen3.5-2b-base/tessl"`, `"backend":"qd metal"`, 1), Unavailable, UnavailableReplyUnparseable},
		{"degraded_is_abstained", degradedAnswer, ModelAbstained, ""},
		{"refused", refused, RequestRefused, "calibration_entry_missing"},
		{"backend_error", backendError, BackendFailed, "deadline_exceeded"},
		{"garbage", "this is not json", Unavailable, UnavailableReplyUnparseable},
		{"empty_line", "", Unavailable, UnavailableReplyUnparseable},
		{"unknown_status", `{"status":"maybe","schema_version":1}`, Unavailable, UnavailableReplyUnparseable},
		{"unknown_envelope_key", strings.Replace(answered, `"degraded":false,"slots"`, `"degraded":false,"extra":1,"slots"`, 1), Unavailable, UnavailableReplyUnparseable},
		{"unknown_slot_key", strings.Replace(answered, `"noul":false,"degraded":false}}}`, `"noul":false,"degraded":false,"x":1}}}`, 1), Unavailable, UnavailableReplyUnparseable},
		{"value_with_noul", strings.Replace(answered, `"score":0.91,"noul":false`, `"score":0.91,"noul":true`, 1), Unavailable, UnavailableReplyUnparseable},
		{"null_value_not_noul", strings.Replace(answered, `"value":"logic"`, `"value":null`, 1), Unavailable, UnavailableReplyUnparseable},
		{"option_outside_set", strings.Replace(answered, `"value":"logic"`, `"value":"verdict"`, 1), Unavailable, UnavailableReplyUnparseable},
		{"conformal_member_outside_set", strings.Replace(answered, `"conformal_set":["logic"]`, `"conformal_set":["pass"]`, 1), Unavailable, UnavailableReplyUnparseable},
		{"null_score", strings.Replace(answered, `"score":0.91`, `"score":null`, 1), Unavailable, UnavailableReplyUnparseable},
		{"score_over_one", strings.Replace(answered, `"score":0.91`, `"score":1.5`, 1), Unavailable, UnavailableReplyUnparseable},
		{"missing_slot", `{"status":"ok","schema_version":1,"backend":"b","degraded":false,"slots":{"defect_class":{"value":"logic","conformal_set":null,"score":0.9,"noul":false,"degraded":false}}}`, Unavailable, UnavailableReplyUnparseable},
		{"span_end_before_start", strings.Replace(answered, `{"start_line":4,"end_line":4}`, `{"start_line":4,"end_line":3}`, 1), Unavailable, UnavailableReplyUnparseable},
		{"schema_version_2", strings.Replace(answered, `"schema_version":1`, `"schema_version":2`, 1), Unavailable, UnavailableReplyUnparseable},
		{"trailing_data", answered + ` {}`, Unavailable, UnavailableReplyUnparseable},
		{"refusal_kind_not_identifier", strings.Replace(refused, `"kind":"calibration_entry_missing"`, `"kind":"file: a/b.go"`, 1), Unavailable, UnavailableReplyUnparseable},
		{"refusal_missing_message", `{"status":"refused","schema_version":1,"refusal":{"kind":"x"}}`, Unavailable, UnavailableReplyUnparseable},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			agent := startAgent(t, reply(tc.reply))
			res := askWith(t, agent.path, 2*time.Second)
			if res.Reading != tc.reading || res.Kind != tc.kind {
				t.Fatalf("got %s/%q, want %s/%q", res.Reading, res.Kind, tc.reading, tc.kind)
			}
			isAnswer := tc.reading == ModelAnswered || tc.reading == ModelAbstained
			if isAnswer != (res.Slots != nil && res.Backend != "") {
				t.Fatalf("slots/backend present=%v for %s", res.Slots != nil, res.Reading)
			}
		})
	}
}

func TestAnAnsweredReplyKeepsItsSlotsAndDropsTheMessage(t *testing.T) {
	agent := startAgent(t, reply(answered))
	res := askWith(t, agent.path, 2*time.Second)
	v, ok := res.Slots[DefectClassSlot].Choice()
	if !ok || v != "logic" || res.Backend != "qd-metal/qwen3.5-2b-base/tessl" {
		t.Fatalf("got %+v", res)
	}
	if got := string(res.Slots[DefectSpanSlot].Value); got != `{"start_line":4,"end_line":4}` {
		t.Fatalf("span value %s", got)
	}
	agent2 := startAgent(t, reply(refused))
	res = askWith(t, agent2.path, 2*time.Second)
	if strings.Contains(res.Kind+res.NotAskedReason+res.Backend, "secret") {
		t.Fatalf("the refusal message leaked into the result: %+v", res)
	}
}

func TestTheAgentReceivesExactlyOneRequestLine(t *testing.T) {
	agent := startAgent(t, reply(refused))
	askWith(t, agent.path, 2*time.Second)
	select {
	case line := <-agent.lines:
		want, err := testRequest().Encode()
		if err != nil {
			t.Fatal(err)
		}
		if !bytes.Equal(line, append(want, '\n')) {
			t.Fatalf("agent got %q, want %q", line, want)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("the agent never received a line")
	}
}

func TestNoSocketIsSocketNotFound(t *testing.T) {
	res := askWith(t, filepath.Join(shortDir(t), "none.sock"), time.Second)
	if res.Reading != Unavailable || res.Kind != UnavailableSocketNotFound {
		t.Fatalf("got %s/%q", res.Reading, res.Kind)
	}
}

func TestASocketNobodyListensOnIsConnectRefused(t *testing.T) {
	path := filepath.Join(shortDir(t), "dead.sock")
	l, err := net.Listen("unix", path)
	if err != nil {
		t.Fatal(err)
	}
	l.(*net.UnixListener).SetUnlinkOnClose(false)
	if err := l.Close(); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(path); err != nil {
		t.Fatalf("the socket file must survive the listener: %v", err)
	}
	res := askWith(t, path, time.Second)
	if res.Reading != Unavailable || res.Kind != UnavailableConnectRefused {
		t.Fatalf("got %s/%q", res.Reading, res.Kind)
	}
}

// TestADrippingAgentCannotHoldTheCallerPastTheDeadline is the §1 deadline
// test: one byte per 150 ms would reset any per-read timeout forever.
func TestADrippingAgentCannotHoldTheCallerPastTheDeadline(t *testing.T) {
	agent := startAgent(t, func(conn net.Conn) {
		for i := 0; i < 100; i++ {
			if _, err := conn.Write([]byte("{")); err != nil {
				return
			}
			time.Sleep(150 * time.Millisecond)
		}
	})
	start := time.Now()
	res := askWith(t, agent.path, 500*time.Millisecond)
	elapsed := time.Since(start)
	if elapsed >= 1500*time.Millisecond {
		t.Fatalf("a 500 ms exchange took %v", elapsed)
	}
	if res.Reading != Unavailable || res.Kind != UnavailableDeadline {
		t.Fatalf("got %s/%q", res.Reading, res.Kind)
	}
}

// TestAReplyWrittenAtAcceptIsReadEvenWhenTheRequestWriteFails is qd serve over
// its connection cap: it writes a reply the moment it accepts and closes
// without reading. A request larger than the socket buffer then fails
// mid-write, but the reply is already buffered and must be the outcome.
// Repeated so the race lands both ways.
func TestAReplyWrittenAtAcceptIsReadEvenWhenTheRequestWriteFails(t *testing.T) {
	big := NewDefectClassRequest(append([]byte("file: a.go\n\n@@ -1 +1 @@\n-a\n+"), bytes.Repeat([]byte("b"), 512*1024)...),
		strings.Repeat("ab", 16))
	for round := 0; round < 20; round++ {
		path := filepath.Join(shortDir(t), "e.sock")
		l, err := net.Listen("unix", path)
		if err != nil {
			t.Fatal(err)
		}
		done := make(chan struct{})
		go func() {
			defer close(done)
			conn, err := l.Accept()
			if err != nil {
				return
			}
			_, _ = conn.Write([]byte(backendError + "\n"))
			_ = conn.Close()
		}()
		c, err := NewClient(path, 2*time.Second)
		if err != nil {
			t.Fatal(err)
		}
		res := c.Ask(context.Background(), big)
		<-done
		if err := l.Close(); err != nil {
			t.Fatal(err)
		}
		if res.Reading != BackendFailed || res.Kind != "deadline_exceeded" {
			t.Fatalf("round %d: got %s/%q, want backend_failed/deadline_exceeded", round, res.Reading, res.Kind)
		}
	}
}

func TestAnAgentThatAcceptsAndNeverWritesIsADeadline(t *testing.T) {
	agent := startAgent(t, func(conn net.Conn) { time.Sleep(3 * time.Second) })
	start := time.Now()
	res := askWith(t, agent.path, 300*time.Millisecond)
	if elapsed := time.Since(start); elapsed > 1500*time.Millisecond {
		t.Fatalf("took %v", elapsed)
	}
	if res.Reading != Unavailable || res.Kind != UnavailableDeadline {
		t.Fatalf("got %s/%q", res.Reading, res.Kind)
	}
}

func TestACloseWithoutANewlineIsClosedWithoutReply(t *testing.T) {
	agent := startAgent(t, func(conn net.Conn) { _, _ = conn.Write([]byte(`{"status":"ok"`)) })
	res := askWith(t, agent.path, 2*time.Second)
	if res.Reading != Unavailable || res.Kind != UnavailableClosedWithoutReply {
		t.Fatalf("got %s/%q", res.Reading, res.Kind)
	}
}

func TestAReplyOverTheCapIsRefusedNotRead(t *testing.T) {
	for _, newline := range []bool{false, true} {
		agent := startAgent(t, func(conn net.Conn) {
			body := bytes.Repeat([]byte("a"), MaxPayloadBytes+10)
			if newline {
				body = append(body, '\n')
			}
			_, _ = conn.Write(body)
		})
		res := askWith(t, agent.path, 5*time.Second)
		if res.Reading != Unavailable || res.Kind != UnavailableReplyOverCap {
			t.Fatalf("newline=%v: got %s/%q", newline, res.Reading, res.Kind)
		}
	}
}

func TestCancellingTheContextEndsTheExchange(t *testing.T) {
	agent := startAgent(t, func(conn net.Conn) { time.Sleep(5 * time.Second) })
	c, err := NewClient(agent.path, MaxExchangeTimeout)
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	time.AfterFunc(100*time.Millisecond, cancel)
	start := time.Now()
	res := c.Ask(ctx, testRequest())
	if elapsed := time.Since(start); elapsed > time.Second {
		t.Fatalf("a cancelled exchange took %v", elapsed)
	}
	if res.Reading != Unavailable || res.Kind != UnavailableDeadline {
		t.Fatalf("got %s/%q", res.Reading, res.Kind)
	}
	// Already cancelled: nothing is sent.
	res = c.Ask(ctx, testRequest())
	if res.Reading != NotAsked {
		t.Fatalf("an already-cancelled ask was sent: %s", res.Reading)
	}
}

func TestARequestOverTheCapIsNeverSent(t *testing.T) {
	agent := startAgent(t, reply(answered))
	c, err := NewClient(agent.path, time.Second)
	if err != nil {
		t.Fatal(err)
	}
	big := NewDefectClassRequest(bytes.Repeat([]byte("x"), MaxPayloadBytes), strings.Repeat("0", 32))
	res := c.Ask(context.Background(), big)
	if res.Reading != NotAsked || res.NotAskedReason != "request_over_cap" {
		t.Fatalf("got %s/%q", res.Reading, res.NotAskedReason)
	}
	time.Sleep(50 * time.Millisecond)
	if n := agent.accepts.Load(); n != 0 {
		t.Fatalf("the agent saw %d connections for a request that must not be sent", n)
	}
}

func TestNewClientRefusesUnboundedOrUnusableSettings(t *testing.T) {
	for _, tc := range []struct {
		path    string
		timeout time.Duration
	}{
		{"", time.Second},
		{"relative.sock", time.Second},
		{"/" + strings.Repeat("d", maxSocketPathBytes), time.Second},
		{"/tmp/a.sock", 0},
		{"/tmp/a.sock", -time.Second},
		{"/tmp/a.sock", MaxExchangeTimeout + time.Millisecond},
	} {
		if _, err := NewClient(tc.path, tc.timeout); err == nil {
			t.Errorf("NewClient(%q, %v) accepted", tc.path, tc.timeout)
		}
	}
}

func TestSocketPathPrefersTheOverride(t *testing.T) {
	env := map[string]string{"HOME": "/Users/x"}
	get := func(k string) string { return env[k] }
	if p, err := SocketPath(get); err != nil || p != "/Users/x/Library/Caches/qd/qd.sock" {
		t.Fatalf("got %q, %v", p, err)
	}
	env[SocketEnv] = "/tmp/q.sock"
	if p, err := SocketPath(get); err != nil || p != "/tmp/q.sock" {
		t.Fatalf("got %q, %v", p, err)
	}
	if _, err := SocketPath(func(string) string { return "" }); err == nil {
		t.Fatal("an unset HOME must not become a relative path")
	}
}

// TestTheReadingsAreTheContractsSix pins the wire spellings to
// caller_record.rs READINGS, in order, and the zero value to invalid.
func TestTheReadingsAreTheContractsSix(t *testing.T) {
	want := []string{"model_answered", "model_abstained", "request_refused", "backend_failed", "unavailable", "not_asked"}
	for i, r := range Readings {
		if !r.Valid() || r.String() != want[i] {
			t.Fatalf("reading %d is %q, want %q", i, r.String(), want[i])
		}
	}
	if Reading(0).Valid() || Reading(7).Valid() {
		t.Fatal("only the six are readings")
	}
}

func TestSlotAnswersEncodeAsTheWireShape(t *testing.T) {
	out, err := json.Marshal(SlotAnswer{Score: 0.1, Noul: true})
	if err != nil {
		t.Fatal(err)
	}
	if string(out) != `{"value":null,"conformal_set":null,"score":0.1,"noul":true,"degraded":false}` {
		t.Fatalf("got %s", out)
	}
}
