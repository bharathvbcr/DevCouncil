//go:build unix

package gussetfn

import (
	"context"
	"time"
)

// DefaultTimeout bounds one policy question. dc-glob is bounded well under
// it; the deadline is for a wedged runtime, which then denies instead of
// hanging the gate.
const DefaultTimeout = 2 * time.Second

// Matcher is policy.Matcher on the engine. The zero value uses
// DefaultTimeout. Its answers equal fnmatch's; an error means the engine
// could not answer, and the gate refuses under its engine rule.
type Matcher struct {
	// Timeout bounds each question; 0 means DefaultTimeout.
	Timeout time.Duration
}

func (m Matcher) timeout() time.Duration {
	if m.Timeout > 0 {
		return m.Timeout
	}
	return DefaultTimeout
}

// MatchAny is fnmatch.MatchAny across the boundary, under the timeout.
func (m Matcher) MatchAny(patterns []string, name string) (bool, error) {
	ctx, cancel := context.WithTimeout(context.Background(), m.timeout())
	defer cancel()
	return MatchAny(ctx, patterns, name)
}

// MatchAnyFold is fnmatch.MatchAnyFold across the boundary, under the timeout.
func (m Matcher) MatchAnyFold(patterns []string, name string) (bool, error) {
	ctx, cancel := context.WithTimeout(context.Background(), m.timeout())
	defer cancel()
	return MatchAnyFold(ctx, patterns, name)
}
