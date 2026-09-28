// Package gussetfn matches paths through dc-glob on the Gusset runtime.
//
// The write gate does not call this. fnmatch.Match returns a bool, and a
// Gusset failure has nowhere to go except "no match" or "match" — one of
// those opens a path the gate meant to refuse, the other denies a write the
// gate meant to allow, and a poisoned handle would do it for every later
// check. Callers that can return an error use Match. The gusset-check
// commands of devcouncil, manvi, jarvis and GitPulse run SelfTest; Manvi's
// serve policy plane runs Check (never a deliberate panic) before answering.
package gussetfn

import (
	"context"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"sync"
	"sync/atomic"
	"unicode/utf8"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/fnmatch"
	"github.com/bharathvbcr/gusset"
)

// maxField matches MAX_FIELD in the umbrella. Both sides refuse a larger
// field so a caller cannot push a multi-megabyte pattern through the inline
// copy on the cgo thread.
const maxField = 1024

// Opcodes the umbrella registers (rust/gusset-engine). Opcode 0 is Match.
const (
	opcodeMatchAny      uint32 = 1
	opcodeSelfTestPanic uint32 = 0x7fff_0001
)

// maxFrame is Gusset's inline-copy limit. A MatchAny list is split into
// frames under it; a bigger []byte would be refused by Call.
const maxFrame = 4096

// maxPatterns matches MAX_PATTERNS in the umbrella.
const maxPatterns = 1024

// poolSize is the shared handle's worker count.
const poolSize = 4

// ErrClosed is returned after Close: the engine is shut for this process.
var ErrClosed = errors.New("gusset: engine is closed")

var (
	registerOnce sync.Once
	// engineMu serialises opening; the hot path reads current without it.
	engineMu sync.Mutex
	current  atomic.Pointer[gusset.Handle]
	closed   atomic.Bool
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
		go func() { _ = h.Close() }()
	}
}

// poisoned reports an error after which h refuses all further work.
func poisoned(err error) bool {
	return errors.Is(err, gusset.ErrPanic) || errors.Is(err, gusset.ErrPoisoned)
}

// Close shuts the shared handle and refuses every later call with ErrClosed.
// A long-running host calls it on shutdown, after gusset.Shutdown if it wants
// the drain bounded.
func Close() error {
	engineMu.Lock()
	defer engineMu.Unlock()
	closed.Store(true)
	h := current.Swap(nil)
	if h == nil {
		return nil
	}
	return h.Close()
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
		if n < len(buf) {
			return total, nil
		}
	}
}

// call runs one frame on the shared handle and retires it on poison.
func call(ctx context.Context, payload []byte) ([]byte, error) {
	h, err := engine()
	if err != nil {
		return nil, err
	}
	out, err := h.Call(ctx, payload)
	if err != nil && poisoned(err) {
		retire(h)
	}
	return out, err
}

// decodeBool reads the engine's one-byte answer.
func decodeBool(out []byte) (bool, error) {
	if len(out) != 1 || (out[0] != 0 && out[0] != 1) {
		return false, fmt.Errorf("gusset: engine returned %q, want a single 0 or 1", out)
	}
	return out[0] == 1, nil
}

func encode(pattern, name string) ([]byte, error) {
	if len(pattern) > maxField || len(name) > maxField {
		return nil, fmt.Errorf("gusset: pattern or name exceeds %d bytes", maxField)
	}
	// Invalid UTF-8 is delivered to the engine, which must return an error
	// rather than panic. Rejecting it here would never exercise that path.
	buf := make([]byte, 4+len(pattern)+len(name))
	binary.LittleEndian.PutUint16(buf[0:2], uint16(len(pattern)))
	copy(buf[2:], pattern)
	off := 2 + len(pattern)
	binary.LittleEndian.PutUint16(buf[off:off+2], uint16(len(name)))
	copy(buf[off+2:], name)
	return buf, nil
}

// Match reports whether name matches pattern under Python fnmatch rules.
//
// A Gusset or engine error is returned, never converted into a boolean.
// The handle is not poisoned by a bad frame: the next well-formed call
// still runs. A handle that is poisoned anyway is retired, and the next call
// opens a new one.
func Match(ctx context.Context, pattern, name string) (bool, error) {
	if ctx == nil {
		return false, errors.New("gusset: nil context")
	}
	payload, err := encode(pattern, name)
	if err != nil {
		return false, err
	}
	out, err := call(ctx, payload)
	if err != nil {
		return false, err
	}
	return decodeBool(out)
}

// MatchAny reports whether name matches any pattern in patterns.
//
// The list crosses the boundary in as few frames as fit Gusset's 4 KiB
// inline copy, one call each, stopping at the first frame that matches. It
// used to be one call per pattern. Every pattern in a frame is validated by
// the engine before any is matched, so a malformed pattern is an error even
// when an earlier one would have matched.
func MatchAny(ctx context.Context, patterns []string, name string) (bool, error) {
	if ctx == nil {
		return false, errors.New("gusset: nil context")
	}
	if len(name) > maxField {
		return false, fmt.Errorf("gusset: name exceeds %d bytes", maxField)
	}
	for _, p := range patterns {
		if len(p) > maxField {
			return false, fmt.Errorf("gusset: pattern exceeds %d bytes", maxField)
		}
	}
	if len(patterns) == 0 {
		// Still a real answer, and still refused on a dead context.
		if err := ctx.Err(); err != nil {
			return false, err
		}
		return false, nil
	}
	actx := gusset.ContextWithOpcode(ctx, opcodeMatchAny)
	for rest := patterns; len(rest) > 0; {
		frame, used := encodeAny(name, rest)
		out, err := call(actx, frame)
		if err != nil {
			return false, err
		}
		matched, err := decodeBool(out)
		if err != nil || matched {
			return matched, err
		}
		rest = rest[used:]
	}
	return false, nil
}

// encodeAny packs name and as many leading patterns as fit one frame, and
// reports how many it took. Fields are length-checked by the caller, so one
// pattern always fits: 2+1024+2+2+1024 is far below maxFrame.
func encodeAny(name string, patterns []string) ([]byte, int) {
	size := 2 + len(name) + 2
	n := 0
	for n < len(patterns) && n < maxPatterns && size+2+len(patterns[n]) <= maxFrame {
		size += 2 + len(patterns[n])
		n++
	}
	buf := make([]byte, 0, size)
	buf = binary.LittleEndian.AppendUint16(buf, uint16(len(name)))
	buf = append(buf, name...)
	buf = binary.LittleEndian.AppendUint16(buf, uint16(n))
	for _, p := range patterns[:n] {
		buf = binary.LittleEndian.AppendUint16(buf, uint16(len(p)))
		buf = append(buf, p...)
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

	if _, err := Match(ctx, "\xff", "a"); err == nil {
		return errors.New("invalid UTF-8 must be refused by the engine")
	} else if errors.Is(err, gusset.ErrPanic) || errors.Is(err, gusset.ErrPoisoned) {
		return fmt.Errorf("invalid UTF-8 poisoned the handle: %w", err)
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
	if !utf8.ValidString("\x01") {
		return errors.New("internal: 0x01 was expected to be valid UTF-8")
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
