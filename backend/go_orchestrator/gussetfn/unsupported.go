//go:build !unix

package gussetfn

import (
	"context"
	"io"
	"time"
)

// Match reports ErrUnsupported: there is no engine on this platform.
func Match(ctx context.Context, pattern, name string) (bool, error) { return false, ErrUnsupported }

// MatchAny reports ErrUnsupported: there is no engine on this platform.
func MatchAny(ctx context.Context, patterns []string, name string) (bool, error) {
	return false, ErrUnsupported
}

// MatchFold reports ErrUnsupported: there is no engine on this platform.
func MatchFold(ctx context.Context, pattern, name string) (bool, error) { return false, ErrUnsupported }

// MatchAnyFold reports ErrUnsupported: there is no engine on this platform.
func MatchAnyFold(ctx context.Context, patterns []string, name string) (bool, error) {
	return false, ErrUnsupported
}

// Matcher is policy.Matcher on the engine; here every answer is
// ErrUnsupported, so a gate given one refuses. Hosts give it to a gate only
// after Check passes, which it never does on this platform.
type Matcher struct{ Timeout time.Duration }

// MatchAny reports ErrUnsupported.
func (Matcher) MatchAny([]string, string) (bool, error) { return false, ErrUnsupported }

// MatchAnyFold reports ErrUnsupported.
func (Matcher) MatchAnyFold([]string, string) (bool, error) { return false, ErrUnsupported }

// Check reports ErrUnsupported: there is no engine on this platform.
func Check(ctx context.Context) error { return ErrUnsupported }

// SelfTest reports ErrUnsupported: there is no engine on this platform.
func SelfTest(ctx context.Context) error { return ErrUnsupported }

// Close has nothing to release on this platform.
func Close() error { return nil }

// Shutdown has nothing to release on this platform.
func Shutdown(time.Duration) error { return nil }

// DrainLogs has nothing to drain on this platform.
func DrainLogs(io.Writer) (int, error) { return 0, nil }
