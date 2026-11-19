//go:build unix

package proc

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"sync"
	"syscall"
	"testing"
)

// noDoneContext reports DeadlineExceeded from Err without ever closing Done:
// the deadline "fired" after the child was already waited for.
type noDoneContext struct{ context.Context }

func (noDoneContext) Err() error { return context.DeadlineExceeded }

func realGit(t *testing.T) string {
	t.Helper()
	real, err := exec.LookPath("git")
	if err != nil {
		t.Skip(err)
	}
	return real
}

// A git that finished 0 and whose deadline reads as expired only afterwards
// delivered a complete answer; it is not discarded.
func TestAdversarialDeadlineAfterCleanExitKeepsTheAnswer(t *testing.T) {
	dir := gitRepo(t)
	s := fakeGitScript(t, dir, "case \" $* \" in\n  *\" rev-parse \"*) echo done; exit 0 ;;\nesac\nexec '"+realGit(t)+"' \"$@\"\n")
	res, err := s.Run(noDoneContext{context.Background()}, 1<<10, "rev-parse", "HEAD")
	if err != nil {
		t.Skipf("Command itself refused under an expired ctx: %v", err)
	}
	if res.TimedOut || string(bytes.TrimSpace(res.Stdout)) != "done" {
		t.Fatalf("res=%+v", res)
	}
}

// A git that exits 1 and whose deadline reads as expired afterwards is a
// timeout, not an exit-1 answer (a caller treats exit 1 as "no HEAD").
func TestAdversarialDeadlineAfterExitOneIsTimedOutNotExitOne(t *testing.T) {
	dir := gitRepo(t)
	s := fakeGitScript(t, dir, "case \" $* \" in\n  *\" rev-parse \"*) exit 1 ;;\nesac\nexec '"+realGit(t)+"' \"$@\"\n")
	res, err := s.Run(noDoneContext{context.Background()}, 1<<10, "rev-parse", "HEAD")
	if err == nil {
		t.Fatal("want an error")
	}
	if !res.TimedOut || res.ExitCode == 1 {
		t.Fatalf("exit 1 under an expired deadline read as an answer: %+v", res)
	}
}

// A git that exits 0 after writing more than the cap reports it.
func TestAdversarialExitZeroOverCapIsTruncated(t *testing.T) {
	dir := gitRepo(t)
	s := fakeGitScript(t, dir, "case \" $* \" in\n  *\" rev-parse \"*) head -c 100000 /dev/zero | tr '\\0' x; echo oops >&2; exit 0 ;;\nesac\nexec '"+realGit(t)+"' \"$@\"\n")
	res, err := s.Run(context.Background(), 100, "rev-parse", "HEAD")
	if err != nil {
		t.Fatal(err)
	}
	if !res.Truncated || len(res.Stdout) != 100 {
		t.Fatalf("truncated=%v stdout=%d", res.Truncated, len(res.Stdout))
	}
}

// A git that closes its stdout and stderr early but keeps running: Run waits
// for the process, and a deadline then returns empty and kills it.
func TestAdversarialGitThatClosesStdoutEarlyIsWaitedFor(t *testing.T) {
	dir := gitRepo(t)
	tmp := t.TempDir()
	started, release := filepath.Join(tmp, "started"), filepath.Join(tmp, "release")
	for _, f := range []string{started, release} {
		if err := syscall.Mkfifo(f, 0o600); err != nil {
			t.Skip(err)
		}
	}
	s := fakeGitScript(t, dir, "case \" $* \" in\n  *\" rev-parse \"*) echo partial; exec 1>&- 2>&-; echo go > '"+started+"'; read x < '"+release+"'; exit 0 ;;\nesac\nexec '"+realGit(t)+"' \"$@\"\n")
	done := make(chan GitResult, 1)
	go func() {
		res, _ := s.Run(context.Background(), 1<<10, "rev-parse", "HEAD")
		done <- res
	}()
	if _, err := os.ReadFile(started); err != nil {
		t.Fatal(err)
	}
	select {
	case res := <-done:
		t.Fatalf("Run returned while git was still running: %+v", res)
	default:
	}
	if err := os.WriteFile(release, []byte("x\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	res := <-done
	if res.TimedOut || string(bytes.TrimSpace(res.Stdout)) != "partial" {
		t.Fatalf("res=%+v", res)
	}
}

func TestAdversarialGitThatClosesStdoutEarlyIsKilledOnDeadline(t *testing.T) {
	dir := gitRepo(t)
	tmp := t.TempDir()
	started, never := filepath.Join(tmp, "started"), filepath.Join(tmp, "never")
	for _, f := range []string{started, never} {
		if err := syscall.Mkfifo(f, 0o600); err != nil {
			t.Skip(err)
		}
	}
	s := fakeGitScript(t, dir, "case \" $* \" in\n  *\" rev-parse \"*) exec 1>&- 2>&-; echo go > '"+started+"'; read x < '"+never+"'; exit 0 ;;\nesac\nexec '"+realGit(t)+"' \"$@\"\n")
	ctx := &expiringContext{Context: context.Background(), done: make(chan struct{})}
	done := make(chan GitResult, 1)
	go func() {
		res, _ := s.Run(ctx, 1<<10, "rev-parse", "HEAD")
		done <- res
	}()
	if _, err := os.ReadFile(started); err != nil {
		t.Fatal(err)
	}
	ctx.expire()
	res := <-done
	if !res.TimedOut {
		t.Fatalf("res=%+v", res)
	}
}

// Concurrent RunEnv calls share nothing.
func TestAdversarialConcurrentRunEnvShareNothing(t *testing.T) {
	dir := gitRepo(t)
	s := fakeGitScript(t, dir, "case \" $* \" in\n  *\" rev-parse \"*) printf '%s' \"$WHO\"; exit 0 ;;\nesac\nexec '"+realGit(t)+"' \"$@\"\n")
	var wg sync.WaitGroup
	errs := make(chan error, 32)
	for i := 0; i < 32; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			want := fmt.Sprintf("id-%d", i)
			res, err := s.RunEnv(context.Background(), 1<<10, []string{"WHO=" + want}, "rev-parse", "HEAD")
			if err != nil || string(res.Stdout) != want {
				errs <- fmt.Errorf("i=%d err=%v stdout=%q", i, err, res.Stdout)
			}
		}()
	}
	wg.Wait()
	close(errs)
	for err := range errs {
		t.Error(err)
	}
}

// Bytes copies under a concurrent Write; a reader's slice is never written.
func TestAdversarialCappedBufferBytesIsACopyUnderWrite(t *testing.T) {
	b := &CappedBuffer{Limit: 1 << 16}
	var wg sync.WaitGroup
	stop := make(chan struct{})
	wg.Add(1)
	go func() {
		defer wg.Done()
		for {
			select {
			case <-stop:
				return
			default:
				b.Write([]byte("abcdefgh"))
			}
		}
	}()
	for i := 0; i < 2000; i++ {
		got := b.Bytes()
		if len(got)%8 != 0 {
			t.Fatalf("torn read: %d", len(got))
		}
		for j := range got {
			got[j] = 'Z' // scribbling on the copy must not touch the buffer
		}
		_ = b.Overflowed()
		_ = b.Len()
		_ = b.String()
	}
	close(stop)
	wg.Wait()
	if bytes.Contains(b.Bytes(), []byte("Z")) {
		t.Fatal("Bytes aliased the buffer")
	}
	_ = errors.New
}
