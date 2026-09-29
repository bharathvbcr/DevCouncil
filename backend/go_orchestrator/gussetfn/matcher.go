//go:build unix

package gussetfn

import (
	"context"
	"sync/atomic"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/fnmatch"
)

// DefaultTimeout is how long one policy question waits for the engine
// before fnmatch answers it instead. An ordinary question takes tens of
// microseconds; this is for a question queued behind slow ones on the
// shared handle's workers, or one that is slow itself.
const DefaultTimeout = 250 * time.Millisecond

// Matcher is policy.Matcher on the engine. The zero value uses
// DefaultTimeout.
//
// Its answers equal fnmatch's for every input. Two outcomes are kept apart:
//
//   - The engine failed — a panic, a poisoned or closed handle, a malformed
//     answer. That is an error, and the gate denies under its engine rule:
//     a broken engine is not asked to guess.
//   - The engine did not answer in time. It is not broken, it is busy, and
//     the answer is fnmatch's, computed here: the reference the engine is
//     held equal to by the CPython fixture and the fuzz differential. An
//     error here would turn a slow Soft denial into a Hard one — a question
//     whose honest answer was "not allowed, demotable" came back
//     "engine unavailable, never demotable" — and let a stream of slow
//     questions deny unrelated ones queued behind them on the four workers.
//
// Fallbacks counts the second case.
type Matcher struct {
	// Timeout bounds the wait for the engine; 0 means DefaultTimeout.
	Timeout time.Duration
}

var fallbacks atomic.Int64

// Fallbacks reports how many policy questions fnmatch answered because the
// engine did not answer within the Matcher's timeout.
func Fallbacks() int64 { return fallbacks.Load() }

func (m Matcher) timeout() time.Duration {
	if m.Timeout > 0 {
		return m.Timeout
	}
	return DefaultTimeout
}

// MatchAny is fnmatch.MatchAny across the boundary.
func (m Matcher) MatchAny(patterns []string, name string) (bool, error) {
	return m.ask(patterns, name, MatchAny, fnmatch.MatchAny)
}

// MatchAnyFold is fnmatch.MatchAnyFold across the boundary.
func (m Matcher) MatchAnyFold(patterns []string, name string) (bool, error) {
	return m.ask(patterns, name, MatchAnyFold, fnmatch.MatchAnyFold)
}

func (m Matcher) ask(
	patterns []string,
	name string,
	engine func(context.Context, []string, string) (bool, error),
	reference func([]string, string) bool,
) (bool, error) {
	ctx, cancel := context.WithTimeout(context.Background(), m.timeout())
	defer cancel()
	ok, err := engine(ctx, patterns, name)
	// Out of time, by either side's clock: Go's wait returns
	// context.DeadlineExceeded, and the engine's own JobContext check returns
	// an engine error ("cancelled: DeadlineExceeded") when Rust notices first.
	// A panic or poison is a broken engine whatever the clock says.
	if err != nil && ctx.Err() != nil && !poisoned(err) {
		fallbacks.Add(1)
		return reference(patterns, name), nil
	}
	return ok, err
}
