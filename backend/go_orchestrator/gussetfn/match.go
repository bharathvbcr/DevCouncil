// Package gussetfn matches paths through dc-glob on the Gusset runtime.
//
// The write gate does not call this. fnmatch.Match returns a bool, and a
// Gusset failure has nowhere to go except "no match" or "match" — one of
// those opens a path the gate meant to refuse, the other denies a write the
// gate meant to allow, and a poisoned handle would do it for every later
// check. Callers that can return an error use Match. The devcouncil and
// manvi gusset-check commands are the production callers.
package gussetfn

import (
	"context"
	"encoding/binary"
	"errors"
	"fmt"
	"sync"
	"unicode/utf8"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/fnmatch"
	"github.com/bharathvbcr/gusset"
)

// maxField matches MAX_FIELD in the umbrella. Both sides refuse a larger
// field so a caller cannot push a multi-megabyte pattern through the inline
// copy on the cgo thread.
const maxField = 1024

var (
	engineOnce sync.Once
	engineH    *gusset.Handle
	engineErr  error
)

func engine() (*gusset.Handle, error) {
	engineOnce.Do(func() {
		register()
		engineH, engineErr = gusset.Open(gusset.WithPoolSize(4))
	})
	if engineErr != nil {
		return nil, engineErr
	}
	if engineH == nil {
		return nil, errors.New("gusset: engine handle is nil")
	}
	return engineH, nil
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
// still runs.
func Match(ctx context.Context, pattern, name string) (bool, error) {
	if ctx == nil {
		return false, errors.New("gusset: nil context")
	}
	h, err := engine()
	if err != nil {
		return false, err
	}
	payload, err := encode(pattern, name)
	if err != nil {
		return false, err
	}
	out, err := h.Call(ctx, payload)
	if err != nil {
		return false, err
	}
	if len(out) != 1 || (out[0] != 0 && out[0] != 1) {
		return false, fmt.Errorf("gusset: engine returned %q, want a single 0 or 1", out)
	}
	return out[0] == 1, nil
}

// MatchAny reports whether name matches any pattern in patterns.
// If any Match call returns an error, matching stops and returns that error.
func MatchAny(ctx context.Context, patterns []string, name string) (bool, error) {
	for _, p := range patterns {
		matched, err := Match(ctx, p, name)
		if err != nil {
			return false, err
		}
		if matched {
			return true, nil
		}
	}
	return false, nil
}

// Check runs the parity vectors the adoption measured, then a frame the
// diagnostic engine would treat as a panic. The second call must still match.
func Check(ctx context.Context) error {
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
	return nil
}
