package proc

import (
	"context"
	"errors"
	"fmt"
	"os/exec"
	"strings"
	"time"
)

// GitResult is one finished git read. Stdout and Stderr hold at most the limit
// Run was given each; Truncated says either stream was cut, so a caller that
// needs the whole answer can refuse the partial one instead of reading it as
// complete.
type GitResult struct {
	Stdout    []byte
	Stderr    []byte
	ExitCode  int
	Truncated bool
	TimedOut  bool
}

// gitWaitDelay bounds how long Wait lingers on a pipe a grandchild still holds
// after git itself has exited.
const gitWaitDelay = 2 * time.Second

// Run runs one git read built by Command and collects its output, capped at
// limit bytes per stream, under the module's subprocess contract: RunBounded
// so a wedged Start cannot outlive ctx, and the process-group isolation
// Command already configured so a deadline reaches git's own children.
//
// It is the one place the host's git reads are executed. devcouncil/diff.go
// and devcouncil/verify/orchestrate.go used to carry a copy each, both on an
// unbounded bytes.Buffer.
//
// The returned error is nil only for a git that exited 0. A non-zero exit is
// the *exec.ExitError with git's stderr appended, so the reason survives into
// whatever the caller reports; ExitCode and the streams are still filled in
// for a caller that treats a particular exit as an answer. A deadline is
// reported as TimedOut with ctx's error. A read that Command refuses, or a git
// that cannot start, is returned as it came: an *exec.Error.
func (s *GitSession) Run(ctx context.Context, limit int, args ...string) (GitResult, error) {
	return s.RunEnv(ctx, limit, nil, args...)
}

// RunEnv is Run with env (KEY=VALUE entries) added to git's environment, for
// the one kind of read that needs it: one that must work against a scratch
// index (GIT_INDEX_FILE) rather than the repository's own. The guards, caps
// and failure classification are Run's.
func (s *GitSession) RunEnv(ctx context.Context, limit int, env []string, args ...string) (GitResult, error) {
	cmd, err := s.Command(ctx, args...)
	if err != nil {
		return GitResult{ExitCode: -1}, err
	}
	cmd.Env = append(cmd.Env, env...)
	stdout := &CappedBuffer{Limit: limit}
	stderr := &CappedBuffer{Limit: limit}
	cmd.Stdout = stdout
	cmd.Stderr = stderr
	cmd.WaitDelay = gitWaitDelay
	err, timedOut := RunBounded(ctx, cmd.Run)
	res := GitResult{
		Stdout:    stdout.Bytes(),
		Stderr:    stderr.Bytes(),
		Truncated: stdout.Overflowed() || stderr.Overflowed(),
		TimedOut:  timedOut,
	}
	if timedOut {
		res.ExitCode = 124
		return res, fmt.Errorf("git %s did not return within the deadline: %w", strings.Join(args, " "), ctx.Err())
	}
	if err == nil {
		return res, nil
	}
	res.ExitCode = -1
	if ctxErr := ctx.Err(); ctxErr != nil {
		res.TimedOut = ctxErr == context.DeadlineExceeded
		if res.TimedOut {
			res.ExitCode = 124
		}
		return res, fmt.Errorf("git %s: %w: %w", strings.Join(args, " "), err, ctxErr)
	}
	var exit *exec.ExitError
	if errors.As(err, &exit) {
		res.ExitCode = exit.ExitCode()
		if detail := strings.TrimSpace(string(res.Stderr)); detail != "" {
			return res, fmt.Errorf("git %s: %w: %s", strings.Join(args, " "), err, detail)
		}
		return res, fmt.Errorf("git %s: %w", strings.Join(args, " "), err)
	}
	return res, err
}
