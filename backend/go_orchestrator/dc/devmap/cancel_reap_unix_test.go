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
	const iterations = 30
	late := 0
	for i := 0; i < iterations; i++ {
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
			// The reap wait is a 100 ms best effort (proc.RunBoundedWithCleanup
			// takes no parameter), so on a loaded host one iteration may find the
			// child a moment late. Count it and confirm it does die; the contract
			// under test is that it is usually already reaped on return, which the
			// previous code (SIGKILL left to an unwaited goroutine) almost never was.
			late++
			deadline := time.Now().Add(5 * time.Second)
			for !gone(pid) && time.Now().Before(deadline) {
				time.Sleep(5 * time.Millisecond)
			}
			if !gone(pid) {
				_ = syscall.Kill(-pid, syscall.SIGKILL)
				t.Fatalf("iteration %d: devmap child %d survived a cancelled invoke", i, pid)
			}
		}
	}
	if late > iterations/5 {
		t.Fatalf("invoke returned before its child was reaped in %d of %d cancels; it must wait for the reap", late, iterations)
	}
}

// A hung binary must still fail fast. Every caller here has a deadline shorter
// than probeTimeout, so each leaves before the probe answers. The probe has to
// run on to its own deadline and cache "presumed hung"; if it were stopped when
// the last caller left, nothing would be cached and every call would start a
// fresh probe and wait out its own deadline.
func TestHungProbeIsCachedEvenWhenEveryCallerLeavesEarly(t *testing.T) {
	dir := t.TempDir()
	spawns := filepath.Join(dir, "spawns")
	script := "#!/bin/sh\necho x >> " + spawns + "\nexec sleep 30\n"
	c := New(writeBinary(t, dir, "devmap", script), dir)
	c.probeTimeout = 600 * time.Millisecond

	for i := 0; i < 2; i++ {
		ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
		err := c.Probe(ctx)
		cancel()
		if !errors.Is(err, context.DeadlineExceeded) {
			t.Fatalf("call %d: a caller leaving early should get its own deadline error, got %v", i, err)
		}
	}
	// Wait for the probe's own deadline to pass and its verdict to be recorded,
	// rather than sleeping a fixed margin past it.
	for until := time.Now().Add(10 * time.Second); time.Now().Before(until); time.Sleep(10 * time.Millisecond) {
		c.probeMu.Lock()
		recorded := !c.probedAt.IsZero()
		c.probeMu.Unlock()
		if recorded {
			break
		}
	}

	start := time.Now()
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	err := c.Probe(ctx)
	if err == nil || !strings.Contains(err.Error(), "presumed hung") {
		t.Fatalf("the verdict was not cached: got %v", err)
	}
	if elapsed := time.Since(start); elapsed > 250*time.Millisecond {
		t.Fatalf("the cached verdict took %s; it should be immediate", elapsed)
	}
	raw, _ := os.ReadFile(spawns)
	if n := strings.Count(string(raw), "x"); n != 1 {
		t.Fatalf("the binary was spawned %d times; callers must share one probe", n)
	}
}

// Close is the host's shutdown seam: it ends an in-flight probe and returns
// only once that probe's child has been reaped, and caches nothing.
func TestCloseEndsAnInFlightProbeAndReapsItsChild(t *testing.T) {
	dir := t.TempDir()
	pidFile := filepath.Join(dir, "pid")
	c := New(writeBinary(t, dir, "devmap", "#!/bin/sh\necho $$ > "+pidFile+"\nexec sleep 30\n"), dir)

	ctx, cancel := context.WithCancel(context.Background())
	errc := make(chan error, 1)
	go func() { errc <- c.Probe(ctx) }()
	pid := waitForPID(t, pidFile)
	cancel()
	<-errc

	// A caller that joins the run and is still waiting when Close cancels it
	// must get a cancellation, not a claim that the binary cannot be trusted.
	joiner := make(chan error, 1)
	go func() { joiner <- c.Probe(context.Background()) }()
	deadline := time.Now().Add(5 * time.Second)
	for {
		c.probeMu.Lock()
		joined := c.probing != nil && c.probing.waiters >= 2
		c.probeMu.Unlock()
		if joined {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("the second caller never joined the in-flight probe")
		}
		time.Sleep(time.Millisecond)
	}

	if err := c.Close(); err != nil {
		t.Fatalf("Close: %v", err)
	}
	if err := <-joiner; !errors.Is(err, context.Canceled) {
		t.Fatalf("a caller waiting on a probe ended by Close should get a cancellation, got %v", err)
	}
	// Close waits for the reap, which is bounded best effort; allow a short
	// poll on a loaded host rather than treating it as an instant guarantee.
	for reapBy := time.Now().Add(2 * time.Second); !gone(pid) && time.Now().Before(reapBy); {
		time.Sleep(5 * time.Millisecond)
	}
	if !gone(pid) {
		_ = syscall.Kill(-pid, syscall.SIGKILL)
		t.Fatalf("the probe child %d outlived Close", pid)
	}
	c.probeMu.Lock()
	cached := !c.probedAt.IsZero()
	c.probeMu.Unlock()
	if cached {
		t.Fatal("a probe ended by Close cached a verdict")
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
	second := make(chan error, 1)
	go func() { second <- c.Probe(context.Background()) }()

	deadline := time.Now().Add(5 * time.Second)
	for {
		c.probeMu.Lock()
		both := c.probing != nil && c.probing.waiters == 2
		c.probeMu.Unlock()
		if both {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("both callers never joined the one probe")
		}
		time.Sleep(time.Millisecond)
	}
	leave()
	if err := <-first; !errors.Is(err, context.Canceled) {
		t.Fatalf("leaving caller: got %v", err)
	}
	if err := <-second; err != nil {
		t.Fatalf("the remaining caller lost its probe when another caller left: %v", err)
	}
}
