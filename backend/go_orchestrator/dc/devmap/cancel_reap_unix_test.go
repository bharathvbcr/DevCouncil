//go:build unix

package devmap

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"
)

// waitForPID returns the pid a stand-in devmap recorded, once it has.
func waitForPID(t *testing.T, path string) int {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		raw, err := os.ReadFile(path)
		if err == nil {
			if pid, convErr := strconv.Atoi(strings.TrimSpace(string(raw))); convErr == nil && pid > 0 {
				return pid
			}
		}
		time.Sleep(5 * time.Millisecond)
	}
	t.Fatalf("the stand-in devmap never recorded its pid at %s", path)
	return 0
}

// gone reports whether pid no longer exists. kill(pid, 0) succeeds on an
// unreaped zombie, so "gone" means the child was reaped, not merely signalled.
func gone(pid int) bool {
	return errors.Is(syscall.Kill(pid, 0), syscall.ESRCH)
}

// A host that cancels an invocation and then exits must not leave the devmap
// child behind: it is a process-group leader, so nothing else will reap it, and
// a build that outlives its host keeps writing .devcouncil/codeintel. invoke
// has to return only once the child is reaped (bounded), not as soon as the
// context fires and a goroutine it does not wait for has been told to kill.
func TestInvokeCancelReapsTheChildBeforeReturning(t *testing.T) {
	for i := 0; i < 15; i++ {
		dir := t.TempDir()
		pidFile := filepath.Join(dir, "pid")
		bin := writeBinary(t, dir, "devmap", "#!/bin/sh\necho $$ > "+pidFile+"\nexec sleep 30\n")
		c := New(bin, dir)

		ctx, cancel := context.WithCancel(context.Background())
		type result struct{ err error }
		done := make(chan result, 1)
		go func() {
			_, err := c.Status(ctx)
			done <- result{err}
		}()
		pid := waitForPID(t, pidFile)
		cancel()

		select {
		case r := <-done:
			if r.err == nil {
				t.Fatalf("iteration %d: a cancelled invocation returned an answer", i)
			}
		case <-time.After(10 * time.Second):
			t.Fatalf("iteration %d: the cancelled invocation never returned", i)
		}
		if !gone(pid) {
			_ = syscall.Kill(-pid, syscall.SIGKILL)
			t.Fatalf("iteration %d: invoke returned on cancel while its devmap child %d was still alive (not reaped)", i, pid)
		}
	}
}

// Probe is shared between concurrent callers and its verdict is cached, so one
// caller's cancellation must never become the binary's verdict. But when every
// caller has gone there is nobody left to answer: the probe must stop and reap
// its child rather than run on for probeTimeout in a host that has moved on,
// and must not cache the abandonment.
func TestProbeAbandonedByAllCallersStopsItsChildAndCachesNothing(t *testing.T) {
	dir := t.TempDir()
	pidFile := filepath.Join(dir, "pid")
	marker := filepath.Join(dir, "second")
	script := "#!/bin/sh\nif [ -e " + marker + " ]; then echo '      --graph-output <PATH>'; exit 0; fi\n" +
		": > " + marker + "\necho $$ > " + pidFile + "\nexec sleep 30\n"
	c := New(writeBinary(t, dir, "devmap", script), dir)

	ctx, cancel := context.WithCancel(context.Background())
	errc := make(chan error, 1)
	go func() { errc <- c.Probe(ctx) }()
	pid := waitForPID(t, pidFile)
	cancel()
	if err := <-errc; !errors.Is(err, context.Canceled) {
		t.Fatalf("the cancelled caller should get its own context error, got %v", err)
	}

	deadline := time.Now().Add(3 * time.Second)
	for !gone(pid) && time.Now().Before(deadline) {
		time.Sleep(10 * time.Millisecond)
	}
	if !gone(pid) {
		_ = syscall.Kill(-pid, syscall.SIGKILL)
		t.Fatalf("the probe child %d outlived every caller that was waiting for it", pid)
	}

	c.probeMu.Lock()
	cached := !c.probedAt.IsZero()
	c.probeMu.Unlock()
	if cached {
		t.Fatal("an abandoned probe cached a verdict about a binary that was never fully asked")
	}
	if err := c.Probe(context.Background()); err != nil {
		t.Fatalf("the probe after an abandoned one failed: %v", err)
	}
}

// One caller leaving must not stop a probe another caller still waits on.
func TestProbeKeepsRunningWhileAnotherCallerWaits(t *testing.T) {
	dir := t.TempDir()
	script := "#!/bin/sh\nsleep 1\necho '      --graph-output <PATH>'\n"
	c := New(writeBinary(t, dir, "devmap", script), dir)

	leaving, leave := context.WithCancel(context.Background())
	first := make(chan error, 1)
	go func() { first <- c.Probe(leaving) }()
	time.Sleep(100 * time.Millisecond)
	second := make(chan error, 1)
	go func() { second <- c.Probe(context.Background()) }()
	time.Sleep(100 * time.Millisecond)
	leave()
	if err := <-first; !errors.Is(err, context.Canceled) {
		t.Fatalf("leaving caller: got %v", err)
	}
	if err := <-second; err != nil {
		t.Fatalf("the remaining caller lost its probe when another caller left: %v", err)
	}
}
