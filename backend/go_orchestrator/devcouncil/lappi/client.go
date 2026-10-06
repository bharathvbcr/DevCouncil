// Package lappi is DevCouncil's client for the Lappi decision agent, written to
// the caller contract (Lappi-decision docs/caller-contract.md).
//
// Three things are fixed here and nowhere else:
//
//   - Transport. One Unix-socket exchange per request: one JSON line out, one
//     JSON line back, all of it under a single deadline. A per-read timeout is
//     not a deadline — an agent that drips one byte at a time would reset it
//     forever — so the whole exchange shares one conn.SetDeadline.
//   - Reading. Every exchange ends in exactly one of the contract's six
//     outcomes (Reading). A reply this client cannot read strictly is
//     Unavailable/reply_unparseable, never a guess, and a degraded answer (a
//     non-model backend) is ModelAbstained.
//   - Admission only. Nothing in this package grants, passes or discharges
//     anything. The verify package may turn a ModelAnswered defect class into
//     one advisory, non-blocking gap; that is the whole of Lappi's reach.
//
// There is no in-process fallback. No socket means Unavailable, and the caller
// keeps its own path.
package lappi

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"math"
	"net"
	"os"
	"path/filepath"
	"regexp"
	"syscall"
	"time"
)

// MaxPayloadBytes is the cap on one request line and on one reply line. It
// mirrors MAX_PAYLOAD_BYTES in Lappi-decision crates/qd-runtime/src/wire.rs:64
// (1_572_864). A request over it is not sent; a reply over it is
// Unavailable/reply_over_cap and is never read into memory whole.
const MaxPayloadBytes = 1_572_864

// MaxExchangeTimeout is the contract's ceiling for a batch caller (§1: "batch
// callers ≤ 10 s"). A verify run is a batch caller.
const MaxExchangeTimeout = 10 * time.Second

// maxSocketPathBytes is the darwin sun_path limit (104, including the NUL).
// A longer path fails inside the kernel with an error that names nothing.
const maxSocketPathBytes = 104

// SocketEnv overrides the socket path (contract §1).
const SocketEnv = "LAPPI_SOCKET"

// defaultSocketRelativeToHome is where `qd serve` and `qd-metal-serve` listen.
const defaultSocketRelativeToHome = "Library/Caches/qd/qd.sock"

// Reading is what a caller read off one exchange, or why there was none. The
// set is closed: these six and no other. The zero value is not a reading, so a
// Result nobody filled in cannot be recorded as one.
type Reading int

const (
	// ModelAnswered: status ok, not degraded, at least one slot not noul.
	ModelAnswered Reading = iota + 1
	// ModelAbstained: status ok and every slot noul, or a degraded answer.
	ModelAbstained
	// RequestRefused: status refused. Kind is the refusal kind.
	RequestRefused
	// BackendFailed: status error. Kind is the backend error kind.
	BackendFailed
	// Unavailable: no usable reply. Kind is one of the Unavailable* reasons.
	Unavailable
	// NotAsked: the request was never sent. NotAskedReason says why.
	NotAsked
)

// Readings lists the six readings in contract order.
var Readings = [6]Reading{ModelAnswered, ModelAbstained, RequestRefused, BackendFailed, Unavailable, NotAsked}

// String is the wire spelling used in caller records.
func (r Reading) String() string {
	switch r {
	case ModelAnswered:
		return "model_answered"
	case ModelAbstained:
		return "model_abstained"
	case RequestRefused:
		return "request_refused"
	case BackendFailed:
		return "backend_failed"
	case Unavailable:
		return "unavailable"
	case NotAsked:
		return "not_asked"
	default:
		return fmt.Sprintf("invalid_reading_%d", int(r))
	}
}

// Valid reports whether r is one of the six.
func (r Reading) Valid() bool { return r >= ModelAnswered && r <= NotAsked }

// The reasons an exchange is Unavailable (contract §2). Cancellation of the
// caller's context is reported as UnavailableDeadline: the contract's list has
// no separate spelling for it, and in both cases the caller stopped waiting.
const (
	UnavailableSocketNotFound     = "socket_not_found"
	UnavailableConnectRefused     = "connect_refused"
	UnavailableDeadline           = "deadline"
	UnavailableClosedWithoutReply = "closed_without_reply"
	UnavailableReplyOverCap       = "reply_over_cap"
	UnavailableReplyUnparseable   = "reply_unparseable"
)

// SlotAnswer is one slot of an ok reply, validated against the slot it answers.
// Its JSON form is the wire form, so a caller record carries it unchanged.
type SlotAnswer struct {
	// Value is null exactly when Noul. For a choice slot it is one of the
	// slot's options; for a span slot it is {"start_line","end_line"}.
	Value        json.RawMessage `json:"value"`
	ConformalSet []string        `json:"conformal_set"`
	Score        float64         `json:"score"`
	Noul         bool            `json:"noul"`
	Degraded     bool            `json:"degraded"`
}

// Choice returns a choice slot's value, or false when the slot abstained or
// is not a choice.
func (s SlotAnswer) Choice() (string, bool) {
	if s.Noul || len(s.Value) == 0 {
		return "", false
	}
	var v string
	if err := json.Unmarshal(s.Value, &v); err != nil {
		return "", false
	}
	return v, true
}

// Result is the outcome of one exchange.
type Result struct {
	Reading Reading
	// Kind is the refusal kind, the backend error kind, or the Unavailable
	// reason; empty for an answer and for NotAsked.
	Kind string
	// NotAskedReason says why a request was not sent. It is the caller's own
	// bookkeeping and is not part of a caller record's lappi block.
	NotAskedReason string
	// Backend and Slots are set only for ModelAnswered and ModelAbstained.
	Backend string
	Slots   map[string]SlotAnswer
	// Latency is the wall time of the exchange, zero when nothing was sent.
	Latency time.Duration
}

func notAsked(reason string) Result { return Result{Reading: NotAsked, NotAskedReason: reason} }

func unavailable(kind string, latency time.Duration) Result {
	return Result{Reading: Unavailable, Kind: kind, Latency: latency}
}

// Client asks one Lappi agent over its Unix socket.
type Client struct {
	socket  string
	timeout time.Duration
}

// NewClient returns a client for socket whose every exchange is bounded by
// timeout, which must be in (0, MaxExchangeTimeout].
func NewClient(socket string, timeout time.Duration) (*Client, error) {
	if socket == "" {
		return nil, errors.New("lappi: empty socket path")
	}
	if !filepath.IsAbs(socket) {
		return nil, fmt.Errorf("lappi: socket path %q is not absolute", socket)
	}
	if len(socket) >= maxSocketPathBytes {
		return nil, fmt.Errorf("lappi: socket path is %d bytes; a Unix socket path must be under %d", len(socket), maxSocketPathBytes)
	}
	if timeout <= 0 || timeout > MaxExchangeTimeout {
		return nil, fmt.Errorf("lappi: exchange timeout %v is outside (0, %v]", timeout, MaxExchangeTimeout)
	}
	return &Client{socket: socket, timeout: timeout}, nil
}

// SocketPath resolves the agent's socket: LAPPI_SOCKET when set, else
// $HOME/Library/Caches/qd/qd.sock. An unset HOME is an error, never a
// relative path.
func SocketPath(getenv func(string) string) (string, error) {
	if override := getenv(SocketEnv); override != "" {
		return override, nil
	}
	home := getenv("HOME")
	if home == "" {
		return "", errors.New("lappi: HOME is unset and " + SocketEnv + " is not given")
	}
	return filepath.Join(home, defaultSocketRelativeToHome), nil
}

// Ask sends req and reads one reply. The exchange is bounded by the client's
// timeout and by ctx, whichever ends first; cancelling ctx ends it promptly.
func (c *Client) Ask(ctx context.Context, req Request) Result {
	line, err := req.Encode()
	if err != nil {
		return notAsked("request_unencodable")
	}
	if len(line) > MaxPayloadBytes {
		return notAsked("request_over_cap")
	}
	if ctx.Err() != nil {
		return notAsked("cancelled")
	}
	start := time.Now()
	deadline := start.Add(c.timeout)
	if d, ok := ctx.Deadline(); ok && d.Before(deadline) {
		deadline = d
	}
	raw, kind := c.exchange(ctx, deadline, line)
	latency := time.Since(start)
	if kind != "" {
		return unavailable(kind, latency)
	}
	res := parseReply(raw, req.Slots)
	res.Latency = latency
	return res
}

// exchange performs connect, write and read under one deadline. It returns the
// reply line without its newline, or an Unavailable reason.
func (c *Client) exchange(ctx context.Context, deadline time.Time, line []byte) ([]byte, string) {
	dialer := net.Dialer{Deadline: deadline}
	conn, err := dialer.DialContext(ctx, "unix", c.socket)
	if err != nil {
		return nil, classifyDial(ctx, err)
	}
	// The exchange is over whichever way it ended; a close error on a socket
	// we only read from carries nothing the caller could act on.
	defer func() { _ = conn.Close() }()
	if err := conn.SetDeadline(deadline); err != nil {
		return nil, UnavailableClosedWithoutReply
	}
	// Cancellation reaches a blocked read or write by closing the connection;
	// classifyIO checks ctx first, so the reason is reported as the deadline
	// and not as the agent hanging up.
	stop := context.AfterFunc(ctx, func() { _ = conn.Close() })
	defer stop()

	framed := make([]byte, 0, len(line)+1)
	framed = append(framed, line...)
	framed = append(framed, '\n')
	// A failed write is not yet the outcome: an agent over its connection cap
	// writes `overloaded` the moment it accepts and closes, so the request can
	// meet a closed socket while that reply sits unread. Read it; only a socket
	// with nothing to read reports the write's failure (Lappi-decision
	// oneshot::ask_over_socket does the same).
	writeFailure := ""
	if n, werr := conn.Write(framed); werr != nil {
		writeFailure = classifyIO(ctx, werr)
	} else if n != len(framed) {
		writeFailure = UnavailableClosedWithoutReply
	}

	reader := bufio.NewReader(io.LimitReader(conn, MaxPayloadBytes+1))
	reply, err := reader.ReadBytes('\n')
	if err != nil && writeFailure != "" {
		return nil, writeFailure
	}
	switch {
	case err == nil:
		reply = reply[:len(reply)-1]
		if len(reply) > MaxPayloadBytes {
			return nil, UnavailableReplyOverCap
		}
		return reply, ""
	case errors.Is(err, io.EOF):
		// LimitReader ends at cap+1 bytes, so a line that long without a
		// newline is over the cap; anything shorter is a hang-up.
		if len(reply) > MaxPayloadBytes {
			return nil, UnavailableReplyOverCap
		}
		if ctx.Err() != nil {
			return nil, UnavailableDeadline
		}
		return nil, UnavailableClosedWithoutReply
	default:
		return nil, classifyIO(ctx, err)
	}
}

func classifyDial(ctx context.Context, err error) string {
	if ctx.Err() != nil || isTimeout(err) {
		return UnavailableDeadline
	}
	if errors.Is(err, fs.ErrNotExist) {
		return UnavailableSocketNotFound
	}
	// ECONNREFUSED, and the rest of what a connect can say (EACCES, ENOTSOCK):
	// there is a path and nothing that will take the request.
	return UnavailableConnectRefused
}

func classifyIO(ctx context.Context, err error) string {
	if ctx.Err() != nil || isTimeout(err) {
		return UnavailableDeadline
	}
	return UnavailableClosedWithoutReply
}

func isTimeout(err error) bool {
	if errors.Is(err, os.ErrDeadlineExceeded) || errors.Is(err, context.DeadlineExceeded) || errors.Is(err, syscall.ETIMEDOUT) {
		return true
	}
	var ne net.Error
	return errors.As(err, &ne) && ne.Timeout()
}

// kindPattern bounds what a refusal or error kind may be. Kinds are
// identifiers (refusal.rs Refusal::kind); anything else is not a kind this
// client can trust into a record.
var kindPattern = regexp.MustCompile(`^[a-z][a-z0-9_]{0,63}$`)

// backendPattern bounds a backend name: printable ASCII without whitespace,
// at most 256 bytes (caller_record.rs opt_str's limit). Names in use include
// "qd-metal/qwen3.5-2b-base/tessl" (qd-metal/src/backend.rs:52) and an
// ensemble's "ensemble/<n>[<member>,...]" (qd-runtime/src/ensemble.rs:297).
var backendPattern = regexp.MustCompile(`^[!-~]{1,256}$`)

var errUnparseable = errors.New("reply unparseable")

func unparseable() Result { return Result{Reading: Unavailable, Kind: UnavailableReplyUnparseable} }

// parseReply reads one reply line strictly: exact key sets at every level,
// schema_version 1, a known status, and slots that match the request.
func parseReply(line []byte, slots []SlotSpec) Result {
	top, err := exactObject(line)
	if err != nil {
		return unparseable()
	}
	var status string
	if err := strictDecode(top["status"], &status); err != nil {
		return unparseable()
	}
	switch status {
	case "ok":
		return parseAnswer(top, slots)
	case "refused":
		return parseFailure(top, "refusal", RequestRefused)
	case "error":
		return parseFailure(top, "error", BackendFailed)
	default:
		return unparseable()
	}
}

func checkSchemaVersion(raw json.RawMessage) error {
	var v int
	if err := strictDecode(raw, &v); err != nil || v != 1 {
		return errUnparseable
	}
	return nil
}

func parseFailure(top map[string]json.RawMessage, field string, reading Reading) Result {
	if requireKeys(top, "status", "schema_version", field, "message") != nil {
		return unparseable()
	}
	if checkSchemaVersion(top["schema_version"]) != nil {
		return unparseable()
	}
	var message string
	if strictDecode(top["message"], &message) != nil {
		return unparseable()
	}
	body, err := exactObject(top[field])
	if err != nil {
		return unparseable()
	}
	var kind string
	if strictDecode(body["kind"], &kind) != nil || !kindPattern.MatchString(kind) {
		return unparseable()
	}
	// The message and the rest of the body are not kept: a refusal echoes
	// excerpts of the context (admission.rs EXCERPT_BYTES), and a record must
	// carry structure only.
	return Result{Reading: reading, Kind: kind}
}

func parseAnswer(top map[string]json.RawMessage, specs []SlotSpec) Result {
	if requireKeys(top, "status", "schema_version", "backend", "degraded", "slots") != nil {
		return unparseable()
	}
	if checkSchemaVersion(top["schema_version"]) != nil {
		return unparseable()
	}
	var backend string
	if strictDecode(top["backend"], &backend) != nil || !backendPattern.MatchString(backend) {
		return unparseable()
	}
	var degraded bool
	if strictDecode(top["degraded"], &degraded) != nil {
		return unparseable()
	}
	rawSlots, err := exactObject(top["slots"])
	if err != nil || len(rawSlots) != len(specs) {
		return unparseable()
	}
	slots := make(map[string]SlotAnswer, len(specs))
	anyAnswered := false
	for _, spec := range specs {
		raw, ok := rawSlots[spec.Name]
		if !ok {
			return unparseable()
		}
		ans, err := parseSlot(raw, spec)
		if err != nil {
			return unparseable()
		}
		if ans.Degraded {
			degraded = true
		}
		if !ans.Noul {
			anyAnswered = true
		}
		slots[spec.Name] = ans
	}
	reading := ModelAbstained
	if anyAnswered && !degraded {
		reading = ModelAnswered
	}
	return Result{Reading: reading, Backend: backend, Slots: slots}
}

func parseSlot(raw json.RawMessage, spec SlotSpec) (SlotAnswer, error) {
	obj, err := exactObject(raw)
	if err != nil {
		return SlotAnswer{}, err
	}
	if err := requireKeys(obj, "value", "conformal_set", "score", "noul", "degraded"); err != nil {
		return SlotAnswer{}, err
	}
	var ans SlotAnswer
	if err := strictDecode(obj["noul"], &ans.Noul); err != nil {
		return SlotAnswer{}, err
	}
	if err := strictDecode(obj["degraded"], &ans.Degraded); err != nil {
		return SlotAnswer{}, err
	}
	if err := strictDecode(obj["score"], &ans.Score); err != nil {
		return SlotAnswer{}, err
	}
	if math.IsNaN(ans.Score) || ans.Score < 0 || ans.Score > 1 {
		return SlotAnswer{}, errUnparseable
	}
	isNull := func(r json.RawMessage) bool { return bytes.Equal(bytes.TrimSpace(r), []byte("null")) }

	// value is absent if and only if noul (schema-api.md, "value is absent if
	// and only if noul"): both illegal combinations are refused.
	valueNull := isNull(obj["value"])
	if valueNull != ans.Noul {
		return SlotAnswer{}, errUnparseable
	}
	switch spec.Type {
	case SlotChoice:
		if !valueNull {
			var v string
			if err := strictDecode(obj["value"], &v); err != nil || !contains(spec.Options, v) {
				return SlotAnswer{}, errUnparseable
			}
			if ans.Value, err = json.Marshal(v); err != nil {
				return SlotAnswer{}, err
			}
		}
		if !isNull(obj["conformal_set"]) {
			var set []string
			if err := strictDecode(obj["conformal_set"], &set); err != nil {
				return SlotAnswer{}, err
			}
			for _, member := range set {
				if !contains(spec.Options, member) {
					return SlotAnswer{}, errUnparseable
				}
			}
			ans.ConformalSet = set
		}
	case SlotSpan:
		if !valueNull {
			span, err := exactObject(obj["value"])
			if err != nil || requireKeys(span, "start_line", "end_line") != nil {
				return SlotAnswer{}, errUnparseable
			}
			var v struct {
				StartLine int `json:"start_line"`
				EndLine   int `json:"end_line"`
			}
			if strictDecode(span["start_line"], &v.StartLine) != nil || strictDecode(span["end_line"], &v.EndLine) != nil {
				return SlotAnswer{}, errUnparseable
			}
			if v.StartLine < 0 || v.EndLine < v.StartLine {
				return SlotAnswer{}, errUnparseable
			}
			if ans.Value, err = json.Marshal(v); err != nil {
				return SlotAnswer{}, err
			}
		}
		// A span has no conformal set (schema-api.md, "null for a span").
		if !isNull(obj["conformal_set"]) {
			return SlotAnswer{}, errUnparseable
		}
	default:
		return SlotAnswer{}, errUnparseable
	}
	return ans, nil
}

func contains(set []string, v string) bool {
	for _, s := range set {
		if s == v {
			return true
		}
	}
	return false
}

// exactObject decodes one JSON object with nothing after it.
func exactObject(raw json.RawMessage) (map[string]json.RawMessage, error) {
	var obj map[string]json.RawMessage
	if err := strictDecode(raw, &obj); err != nil {
		return nil, err
	}
	if obj == nil {
		return nil, errUnparseable
	}
	return obj, nil
}

// requireKeys refuses an object whose key set is not exactly keys.
func requireKeys(obj map[string]json.RawMessage, keys ...string) error {
	if len(obj) != len(keys) {
		return errUnparseable
	}
	for _, k := range keys {
		if _, ok := obj[k]; !ok {
			return errUnparseable
		}
	}
	return nil
}

// strictDecode decodes raw into v, refusing null, trailing data and, for
// structs, unknown fields. encoding/json decodes null into a scalar as a
// silent no-op, which would read "score": null as a score of 0; every field
// that may be null is checked for it before this is called.
func strictDecode(raw json.RawMessage, v any) error {
	trimmed := bytes.TrimSpace(raw)
	if len(trimmed) == 0 || bytes.Equal(trimmed, []byte("null")) {
		return errUnparseable
	}
	dec := json.NewDecoder(bytes.NewReader(raw))
	dec.DisallowUnknownFields()
	if err := dec.Decode(v); err != nil {
		return err
	}
	if _, err := dec.Token(); !errors.Is(err, io.EOF) {
		return errUnparseable
	}
	return nil
}
