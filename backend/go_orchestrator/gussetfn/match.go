//go:build unix

// Package gussetfn matches paths through dc-glob on the Gusset runtime.
//
// Every entry point answers exactly what fnmatch answers for the same
// arguments, or an error. The error is why the policy gates can use it:
// Matcher satisfies policy.Matcher, and a gate turns an error into a denial
// under path.engine_unavailable or command.engine_unavailable instead of
// guessing a bool. A Gusset failure has no honest bool — "no match" opens a
// path the gate meant to refuse, "match" denies a write it meant to allow.
// The gusset-check commands of devcouncil, manvi, jarvis and GitPulse run
// SelfTest; Manvi's serve runs Check (never a deliberate panic) before it
// hands its gates a Matcher.
package gussetfn

import (
	"context"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"strings"
	"sync"
	"sync/atomic"
	"time"
	"unicode/utf8"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/fnmatch"
	"github.com/bharathvbcr/gusset"
)

// maxField matches MAX_FIELD in the umbrella: fnmatch's 16384-rune cap at
// four bytes a rune. A field fnmatch would match always fits it.
const maxField = 16_384 * 4

// Opcodes the umbrella registers (rust/gusset-engine). Opcode 0 is the
// single-pattern frame, which only tests send now: every entry point here is
// a list of one or more.
const (
	opcodeMatch         uint32 = 0
	opcodeMatchAny      uint32 = 1
	opcodeSelfTestPanic uint32 = 0x7fff_0001
)

// maxFrame is Gusset's inline-copy limit. A frame over it travels in a
// Rust-owned buffer (roundTrip); a bigger []byte would be refused by Call.
const maxFrame = 4096

// maxFrameBytes bounds one frame, so a very long list is several bounded
// crossings rather than one allocation of its whole size.
const maxFrameBytes = 1 << 20

// maxPatterns matches MAX_PATTERNS in the umbrella.
const maxPatterns = 1024

// poolSize is the shared handle's worker count.
const poolSize = 4

var (
	registerOnce sync.Once
	// engineMu serialises opening; the hot path reads current without it.
	engineMu sync.Mutex
	current  atomic.Pointer[gusset.Handle]
	closed   atomic.Bool
	// opens counts handles opened, so tests can tell one replacement from a
	// stampede of them.
	opens atomic.Int64
)

// engine returns the shared handle, opening one if there is none.
//
// This used to be a sync.Once. A poisoned handle then stayed the only handle
// for the life of the process, and every later Match answered ErrPoisoned —
// the "close it and open a new handle" Gusset asks for had nowhere to happen.
// A failed Open is also retried by the next call instead of being latched.
func engine() (*gusset.Handle, error) {
	if h := current.Load(); h != nil {
		return h, nil
	}
	engineMu.Lock()
	defer engineMu.Unlock()
	if closed.Load() {
		return nil, ErrClosed
	}
	if h := current.Load(); h != nil {
		return h, nil
	}
	registerOnce.Do(register)
	h, err := gusset.Open(gusset.WithPoolSize(poolSize))
	if err != nil {
		return nil, err
	}
	opens.Add(1)
	current.Store(h)
	return h, nil
}

// retire drops h after it reported poison, so the next call opens a fresh
// handle. Only the first caller to see a given handle's poison closes it.
//
// Close joins h's workers. dc-glob is bounded, so that is short, but it runs
// off the caller's goroutine anyway: the caller already has its answer and
// its deadline is not Close's business.
func retire(h *gusset.Handle) {
	if current.CompareAndSwap(h, nil) {
		go closeHandle(h)
	}
}

// bufferMem orders touching a Rust-owned buffer's bytes against closing the
// handle that owns them. Handle.Close frees every buffer of the handle, and
// the slice Buffer.Bytes returned stays pointed at that memory: a copy into
// it racing a retire from a sibling's panic wrote into freed Rust memory.
// Readers hold it only for the copy, never across a call, so a close waits
// microseconds, not for work.
var bufferMem sync.RWMutex

func closeHandle(h *gusset.Handle) error {
	bufferMem.Lock()
	defer bufferMem.Unlock()
	return h.Close()
}

// poisoned reports an error after which h refuses all further work.
func poisoned(err error) bool {
	return errors.Is(err, gusset.ErrPanic) || errors.Is(err, gusset.ErrPoisoned)
}

// Close shuts the shared handle and refuses every later call with ErrClosed.
//
// It joins the handle's workers without a time limit, so a host that must
// not hang on a stuck engine calls Shutdown instead.
func Close() error {
	engineMu.Lock()
	defer engineMu.Unlock()
	closed.Store(true)
	h := current.Swap(nil)
	if h == nil {
		return nil
	}
	return closeHandle(h)
}

// Shutdown is Close with a bound: it runs gusset.Shutdown(drain) first, which
// refuses new work process-wide and cancels every job, then joins the handle.
// When the drain expires with work still running (an engine that ignores its
// JobContext) it returns that error without joining, because the join would
// wait for exactly that work: the process is exiting and the OS reclaims the
// threads. Process-wide and one-way, like gusset.Shutdown: for process exit
// only.
func Shutdown(drain time.Duration) error {
	engineMu.Lock()
	closed.Store(true)
	engineMu.Unlock()
	if err := gusset.Shutdown(drain); err != nil {
		current.Store(nil)
		return err
	}
	return Close()
}

// DrainLogs writes whatever Gusset's Rust side logged (worker respawns,
// sigaltstack failures, caught panics) to w and reports how many bytes it
// wrote. Nothing reads that ring otherwise; it is bounded and evicts the
// oldest line, so a host that never drains it loses the evidence.
func DrainLogs(w io.Writer) (int, error) {
	buf := make([]byte, 64<<10)
	total := 0
	for {
		n := gusset.DrainLogs(buf)
		if n <= 0 {
			return total, nil
		}
		if n > len(buf) {
			n = len(buf)
		}
		m, err := w.Write(buf[:n])
		total += m
		if err != nil {
			return total, err
		}
		// Not "a short read means empty": gusset drains whole lines and never
		// splits a UTF-8 character, so a short read can leave more behind.
		// Only 0 means the ring is empty.
	}
}

// handleGone reports an error that says h was closed under the call:
// retired after another call's panic, or shut by Close. gusset.ErrClosed
// matches every such error; this used to match its two message texts.
func handleGone(err error) bool {
	return errors.Is(err, gusset.ErrClosed)
}

// call runs one frame on the shared handle and retires it on poison.
//
// A call that lands on a handle another call just poisoned or retired
// retries once on the replacement: a match is idempotent, and the failure
// was a sibling's, not this frame's. Before, those callers got ErrPoisoned or
// an unclassified "handle is closed" while a healthy handle was one call
// away. ErrPanic is never retried — this frame is what panicked, and a retry
// would poison the replacement too. After Close the retry reaches engine(),
// so a call racing Close answers ErrClosed, as Close promises.
func call(ctx context.Context, payload []byte) ([]byte, error) {
	for attempt := 0; ; attempt++ {
		h, err := engine()
		if err != nil {
			return nil, err
		}
		out, err := roundTrip(ctx, h, payload)
		if err == nil {
			return out, nil
		}
		if poisoned(err) {
			retire(h)
		}
		sibling := errors.Is(err, gusset.ErrPoisoned) || handleGone(err)
		if !sibling {
			return nil, err
		}
		// After Close the retry reaches engine(), which answers ErrClosed.
		if attempt > 0 || ctx.Err() != nil {
			return nil, err
		}
	}
}

// answer reads the engine's one-byte answer: 0 no match, 1 match, 2
// undecided (dc-glob's None — past its cap or out of step budget).
func answer(out []byte) (byte, error) {
	if len(out) != 1 || out[0] > undecided {
		return 0, fmt.Errorf("gusset: engine returned %q, want a single 0, 1 or 2", out)
	}
	return out[0], nil
}

// Answer bytes, matching NO_MATCH, MATCH and UNDECIDED in the umbrella.
const (
	noMatch   byte = 0
	match     byte = 1
	undecided byte = 2
)

// normalize gives the engine the text Go's matcher walks. fnmatch converts
// with []rune, which reads each invalid UTF-8 byte as U+FFFD; the engine
// takes &str and refuses invalid UTF-8. strings.ToValidUTF8 would collapse a
// run of invalid bytes into one U+FFFD and shift every position after it, so
// "[!a]?\xff\xff" would be answered for a different name.
func normalize(s string) string {
	if utf8.ValidString(s) {
		return s
	}
	return string([]rune(s))
}

func appendField(buf []byte, s string) []byte {
	buf = binary.LittleEndian.AppendUint32(buf, uint32(len(s)))
	return append(buf, s...)
}

// roundTrip sends one frame on h: inline under Gusset's 4 KiB copy limit,
// through a Rust-owned buffer above it. A long command or path must reach the
// engine, because Go's matcher answers for it; the old 1 KiB field cap made
// those an error, which the gate would have turned into a denial.
func roundTrip(ctx context.Context, h *gusset.Handle, frame []byte) ([]byte, error) {
	if len(frame) <= maxFrame {
		return h.Call(ctx, frame)
	}
	in, err := h.NewBuffer(len(frame))
	if err != nil {
		return nil, err
	}
	defer in.Free()
	if !withBuffer(in, func(dst []byte) bool {
		if len(dst) != len(frame) {
			return false
		}
		copy(dst, frame)
		return true
	}) {
		// Bytes is nil once the handle is closed under us; classify it as
		// that, so call retries on the replacement handle.
		return nil, fmt.Errorf("gusset: input buffer unavailable: %w", gusset.ErrClosed)
	}
	out, err := h.CallBuffer(ctx, in)
	if err != nil {
		return nil, err
	}
	defer out.Free()
	var result []byte
	if !withBuffer(out, func(src []byte) bool {
		result = append([]byte(nil), src...)
		return src != nil
	}) {
		// The same close, after the call: every answer is one byte, so a nil
		// result is a handle closed under us, not an empty answer. Before,
		// it reached answer() as "engine returned \"\"", which is not
		// ErrClosed, so the gate denied instead of retrying.
		return nil, fmt.Errorf("gusset: result buffer unavailable: %w", gusset.ErrClosed)
	}
	return result, nil
}

// withBuffer runs f on b's bytes with the handle kept open for the duration.
func withBuffer(b *gusset.Buffer, f func([]byte) bool) bool {
	bufferMem.RLock()
	defer bufferMem.RUnlock()
	return f(b.Bytes())
}

// Match reports whether name matches pattern, exactly as fnmatch.Match does.
//
// Every input Go answers is answered: invalid UTF-8 is normalized the way
// fnmatch reads it, and an input past fnmatch's cap is no match, decided here
// without a crossing. Only a Gusset or engine failure is an error, and it is
// returned, never converted into a boolean. The handle is not poisoned by a
// bad frame; a handle that is poisoned anyway is retired, and the next call
// opens a new one.
func Match(ctx context.Context, pattern, name string) (bool, error) {
	return MatchAny(ctx, []string{pattern}, name)
}

// MatchFold is fnmatch.MatchFold across the boundary: case-folded, and an
// input the matcher cannot decide counts as a match, because MatchFold is
// the deny-list entry point and a false there lets a write through.
func MatchFold(ctx context.Context, pattern, name string) (bool, error) {
	return MatchAnyFold(ctx, []string{pattern}, name)
}

// MatchAny reports whether name matches any pattern, as fnmatch.MatchAny does.
//
// The list crosses in frames of at most maxPatterns patterns and
// maxFrameBytes bytes, one call each, stopping at the first frame that
// matches. A frame over 4 KiB travels in a Rust-owned buffer.
func MatchAny(ctx context.Context, patterns []string, name string) (bool, error) {
	return ask(ctx, patterns, name, false)
}

// MatchAnyFold is fnmatch.MatchAnyFold across the boundary; see MatchFold.
func MatchAnyFold(ctx context.Context, patterns []string, name string) (bool, error) {
	return ask(ctx, patterns, name, true)
}

// ask is the one path for all four entry points.
//
// Go decides what it can decide without the engine, with fnmatch's own
// rules, so the two can only differ in the walk itself: an oversized name or
// pattern is no match case-sensitively and a match case-folded, and folding
// is fnmatch.Fold, so Unicode case rules are Go's on both paths. An UNDECIDED
// answer (the step budget, which both sides compute identically) reads the
// same way.
func ask(ctx context.Context, patterns []string, name string, fold bool) (bool, error) {
	if ctx == nil {
		return false, errors.New("gusset: nil context")
	}
	if err := ctx.Err(); err != nil {
		// Refused on a dead context even when the answer needs no crossing.
		return false, err
	}
	if len(patterns) == 0 {
		return false, nil
	}
	if fnmatch.Oversized(name) {
		return fold, nil
	}
	kept := make([]string, 0, len(patterns))
	for _, p := range patterns {
		if fnmatch.Oversized(p) {
			if fold {
				return true, nil
			}
			continue
		}
		if fold {
			p = fnmatch.Fold(p)
		}
		kept = append(kept, normalize(p))
	}
	if fold {
		name = fnmatch.Fold(name)
	}
	name = normalize(name)

	// Opcode 1, whatever the caller's context carries. gusset reads the
	// opcode from ctx, so a ctx that had passed through another opcode — the
	// self-test's — would decode this frame as that, or panic the shared
	// handle.
	actx := gusset.ContextWithOpcode(ctx, opcodeMatchAny)
	for rest := kept; len(rest) > 0; {
		frame, used := encodeAny(name, rest)
		out, err := call(actx, frame)
		if err != nil {
			return false, err
		}
		a, err := answer(out)
		if err != nil {
			return false, err
		}
		if a == match || (a == undecided && fold) {
			return true, nil
		}
		rest = rest[used:]
	}
	return false, nil
}

// encodeAny packs name and as many leading patterns as fit one frame, and
// reports how many it took. One pattern always fits: fields are under
// fnmatch's cap, far below maxFrameBytes.
func encodeAny(name string, patterns []string) ([]byte, int) {
	size := 4 + len(name) + 4
	n := 0
	for n < len(patterns) && n < maxPatterns && (n == 0 || size+4+len(patterns[n]) <= maxFrameBytes) {
		size += 4 + len(patterns[n])
		n++
	}
	buf := make([]byte, 0, size)
	buf = appendField(buf, name)
	buf = binary.LittleEndian.AppendUint32(buf, uint32(n))
	for _, p := range patterns[:n] {
		buf = appendField(buf, p)
	}
	return buf, n
}

// Check runs the parity vectors the adoption measured, the batched MatchAny
// path, and a frame the diagnostic engine would treat as a panic. It never
// panics anything: Manvi runs it on the first policy check of a live server,
// where a deliberate "panicked at" on stderr reads as a crash. SelfTest adds
// the panic proof for the gusset-check commands.
func Check(ctx context.Context) error {
	if ctx == nil {
		return errors.New("gusset: nil context")
	}
	vectors := []struct {
		pattern, name string
		want          bool
	}{
		{"*.py", "src/foo.py", true},
		{"*.py", "src/foo.rs", false},
		{"*", "src/foo.py", true},
		{"src/*.py", "src/foo.py", true},
	}
	for _, v := range vectors {
		got, err := Match(ctx, v.pattern, v.name)
		if err != nil {
			return fmt.Errorf("Match(%q, %q): %w", v.pattern, v.name, err)
		}
		if got != v.want {
			return fmt.Errorf("Match(%q, %q) = %v, want %v", v.pattern, v.name, got, v.want)
		}
		if goGot := fnmatch.Match(v.pattern, v.name); goGot != got {
			return fmt.Errorf("gusset and Go fnmatch disagree on (%q, %q): gusset %v, go %v", v.pattern, v.name, got, goGot)
		}
	}

	// A frame the engine refuses is an error, not a panic: the handle stays
	// usable. Go never builds one (invalid UTF-8 is normalized first), so it
	// is sent raw, on opcode 0.
	bad := binary.LittleEndian.AppendUint32(nil, 1)
	bad = append(bad, 0xff)
	bad = appendField(bad, "a")
	if _, err := call(gusset.ContextWithOpcode(ctx, opcodeMatch), bad); err == nil {
		return errors.New("a frame with invalid UTF-8 must be refused by the engine")
	} else if poisoned(err) {
		return fmt.Errorf("a refused frame poisoned the handle: %w", err)
	}

	// Inputs Go answers without the engine's help must get Go's answer:
	// invalid UTF-8 read as U+FFFD per byte, case folding, and a frame over
	// the 4 KiB inline copy.
	long := strings.Repeat("a/", 3000) + "Secret.PEM"
	for _, v := range []struct {
		pattern, name string
		fold          bool
	}{
		{"?\xff?", "\xff\xff\xff", false},
		{"[!a]", "\xff", false},
		{"*.pem", "keys/ID.PEM", true},
		{"*.pem", long, true},
		{"*.PEM", long, false},
	} {
		got, err := ask(ctx, []string{v.pattern}, v.name, v.fold)
		if err != nil {
			return fmt.Errorf("ask(%q, %.40q, fold=%v): %w", v.pattern, v.name, v.fold, err)
		}
		want := fnmatch.Match(v.pattern, v.name)
		if v.fold {
			want = fnmatch.MatchFold(v.pattern, v.name)
		}
		if got != want {
			return fmt.Errorf("gusset and Go fnmatch disagree on (%q, %.40q, fold=%v): gusset %v, go %v", v.pattern, v.name, v.fold, got, want)
		}
	}

	// A payload whose first byte is 1 panics the diagnostic engine. With the
	// adopter registered, it is a one-byte pattern, not an opcode.
	got, err := Match(ctx, "\x01", "\x01")
	if err != nil {
		return fmt.Errorf("literal 0x01 after a bad frame: %w", err)
	}
	if !got {
		return errors.New("literal 0x01 did not match itself")
	}

	anyPatterns := []string{"*.rs", "*.go", "src/*.py"}
	if got, err := MatchAny(ctx, anyPatterns, "src/foo.py"); err != nil || !got {
		return fmt.Errorf("MatchAny(%q, src/foo.py) = %v, %v; want true", anyPatterns, got, err)
	}
	if got, err := MatchAny(ctx, anyPatterns, "src/foo.js"); err != nil || got {
		return fmt.Errorf("MatchAny(%q, src/foo.js) = %v, %v; want false", anyPatterns, got, err)
	}

	return nil
}

// SelfTest is Check plus a real panic inside this archive on a throwaway
// handle (I2), after which the shared handle must still match. Rust's panic
// hook prints the induced panic to stderr; that line is the proof running,
// not a failure. For the gusset-check commands and tests, not for a server.
func SelfTest(ctx context.Context) error {
	if err := Check(ctx); err != nil {
		return err
	}
	if err := checkPanicFirewall(ctx); err != nil {
		return err
	}
	got, err := Match(ctx, "*.py", "src/foo.py")
	if err != nil || !got {
		return fmt.Errorf("shared handle after the self-test panic: Match = %v, %v", got, err)
	}
	return nil
}

// checkPanicFirewall panics inside the umbrella on a handle of its own and
// requires the panic to come back as ErrPanic, the handle to refuse the next
// call as ErrPoisoned, and the process to still be here. The diagnostic
// engine proves this for Gusset; this proves it for the archive this binary
// actually links, built with this crate's panic strategy.
func checkPanicFirewall(ctx context.Context) error {
	if _, err := engine(); err != nil { // registers the umbrella's engines
		return err
	}
	h, err := gusset.Open(gusset.WithPoolSize(1), gusset.WithOpcode(opcodeSelfTestPanic))
	if err != nil {
		return fmt.Errorf("self-test handle: %w", err)
	}
	defer h.Close()
	if _, err := h.Call(ctx, nil); !errors.Is(err, gusset.ErrPanic) {
		return fmt.Errorf("self-test panic returned %v, want ErrPanic", err)
	}
	if _, err := h.Call(ctx, nil); !errors.Is(err, gusset.ErrPoisoned) {
		return fmt.Errorf("call after a panic returned %v, want ErrPoisoned", err)
	}
	return nil
}
