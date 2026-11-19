//go:build unix

package proc

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"syscall"
	"testing"
	"time"
)

// expiringContext is a context whose deadline the test fires by hand, so the
// deadline arrives at a point the test chose rather than after a wall-clock
// wait. Err reports DeadlineExceeded once it has fired.
type expiringContext struct {
	context.Context
	done chan struct{}
}

func (c *expiringContext) Done() <-chan struct{} { return c.done }

func (c *expiringContext) Err() error {
	select {
	case <-c.done:
		return context.DeadlineExceeded
	default:
		return nil
	}
}

func (c *expiringContext) expire() { close(c.done) }

// A read that the deadline wins must come back empty. RunBounded abandons the
// goroutine running git, so git may still be writing into the output buffers;
// the result must neither carry a partial answer nor read the buffers while
// they are written. The test makes the deadline deterministic: the fake git
// signals through a FIFO that it is about to write, the test then fires the deadline, and
// `go test -race` is what proves the buffers were not read concurrently.
func TestGitSessionRunReturnsNothingWhenTheDeadlineWinsOverAWritingGit(t *testing.T) {
	dir := gitRepo(t)
	fifo := filepath.Join(t.TempDir(), "started")
	if err := syscall.Mkfifo(fifo, 0o600); err != nil {
		t.Skip(err)
	}
	real, err := exec.LookPath("git")
	if err != nil {
		t.Skip(err)
	}
	// Only rev-parse misbehaves: Command reads the repository config through git
	// before it builds a read, and that read must reach the real one.
	s := fakeGitScript(t, dir, "case \" $* \" in\n  *\" rev-parse \"*) head -c 65536 /dev/zero | tr '\\0' x; echo go > '"+fifo+"'; exec yes ;;\nesac\nexec '"+real+"' \"$@\"\n")

	ctx := &expiringContext{Context: context.Background(), done: make(chan struct{})}
	type outcome struct {
		res GitResult
		err error
	}
	done := make(chan outcome, 1)
	go func() {
		res, err := s.Run(ctx, 1<<20, "rev-parse", "--verify", "HEAD")
		done <- outcome{res, err}
	}()
	// A fake that never writes must fail the test with a message, not hang it
	// until go test -timeout. The bound is generous and is only a failure bound:
	// the test never waits on it when the fake behaves.
	signalled := make(chan error, 1)
	go func() {
		_, err := os.ReadFile(fifo)
		signalled <- err
	}()
	select {
	case err := <-signalled:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(2 * time.Minute):
		t.Fatal("the fake git never signalled that it was writing")
	}
	ctx.expire()
	var got outcome
	select {
	case got = <-done:
	case <-time.After(2 * time.Minute):
		t.Fatal("Run did not return after its deadline fired")
	}

	if got.err == nil {
		t.Fatal("a read the deadline cut short must be an error")
	}
	if !got.res.TimedOut || got.res.ExitCode != 124 {
		t.Fatalf("TimedOut=%v ExitCode=%d, want TimedOut and 124", got.res.TimedOut, got.res.ExitCode)
	}
	if len(got.res.Stdout) != 0 || len(got.res.Stderr) != 0 || got.res.Truncated {
		t.Fatalf("a timed-out read returned bytes: stdout=%d stderr=%d truncated=%v", len(got.res.Stdout), len(got.res.Stderr), got.res.Truncated)
	}
}

// A caller's GIT_DIR / GIT_WORK_TREE (a hook, an alias, a wrapper) must not
// redirect a read of the session's root to another repository.
func TestGitSessionIgnoresTheCallersRepositoryLocatingEnvironment(t *testing.T) {
	root := gitRepo(t)
	other := gitRepo(t)
	if err := os.WriteFile(filepath.Join(root, "mine.txt"), []byte("x\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	t.Setenv("GIT_DIR", filepath.Join(other, ".git"))
	t.Setenv("GIT_WORK_TREE", other)
	t.Setenv("GIT_INDEX_FILE", filepath.Join(other, ".git", "index"))
	s, err := NewGitSession(root)
	if err != nil {
		t.Fatal(err)
	}
	res, err := s.Run(context.Background(), 1<<20, "ls-files", "--others", "-z")
	if err != nil {
		t.Fatal(err)
	}
	if string(res.Stdout) != "mine.txt\x00" {
		t.Fatalf("read %q, want the session root's mine.txt", res.Stdout)
	}
}
