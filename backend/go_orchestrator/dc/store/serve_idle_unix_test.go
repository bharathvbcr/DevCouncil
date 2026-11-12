//go:build unix

package store

import (
	"context"
	"errors"
	"math/rand/v2"
	"os"
	"path/filepath"
	"sync"
	"syscall"
	"testing"
	"time"
)

// reapDeadline bounds how long a test waits, past the TTL, for an expired child
// to be gone. Generous on purpose: reaping is a timer and a wait for the
// process, and this host runs at load 50–120 while suites race beside it. Only
// the failure path ever waits this long.
const reapDeadline = 2 * time.Minute

// processDone reports whether the child's process has exited and been waited
// for. It asks the os.Process rather than kill(pid, 0): once waited for, Go
// answers ErrProcessDone without touching the pid, so a pid the system has
// since reused for something else cannot make a reaped child look alive.
func processDone(child *servedChild) bool {
	return errors.Is(child.cmd.Process.Signal(syscall.Signal(0)), os.ErrProcessDone)
}

func waitProcessDone(t *testing.T, child *servedChild, what string) {
	t.Helper()
	deadline := time.Now().Add(reapDeadline)
	for !processDone(child) {
		if time.Now().After(deadline) {
			t.Fatalf("%s: the store process was still running %s after it should have ended", what, reapDeadline)
		}
		time.Sleep(20 * time.Millisecond)
	}
}

func idleCount(c *Client) int {
	c.pool.mu.Lock()
	defer c.pool.mu.Unlock()
	return len(c.pool.idle)
}

// TestAClientNobodyClosesHoldsNoStoreProcessPastItsIdleTTL is the leak.
//
// A Manvi test binary built a client in every fixture and closed none. Before
// the idle TTL each one kept its warm child until the binary exited, and a
// `-count=200` run held 7,517 `dcstore serve` processes three hours in. The
// body below never calls Close; the cleanup only keeps a failing run from
// leaking the same way.
func TestAClientNobodyClosesHoldsNoStoreProcessPastItsIdleTTL(t *testing.T) {
	c := New(binary(t), filepath.Join(t.TempDir(), "state.sqlite"))
	t.Cleanup(c.Close)
	c.pool.idleTTL = 3 * time.Second
	seedTask(t, c, "TASK-1")
	ctx := context.Background()

	if _, err := c.Active(ctx, "TASK-1"); err != nil {
		t.Fatalf("active: %v", err)
	}
	child := idleChild(t, c)

	// Control: inside its TTL the child is alive and parked, so a pass below
	// cannot come from a child that died some other way.
	c.pool.mu.Lock()
	young := time.Since(child.idleSince) < c.pool.idleTTL
	c.pool.mu.Unlock()
	if young && processDone(child) {
		t.Fatal("control: the store process ended inside its idle TTL")
	}

	waitProcessDone(t, child, "an idle child of an unclosed client")
	if n := idleCount(c); n != 0 {
		t.Fatalf("%d reaped child(ren) are still listed as idle", n)
	}

	// Reaping retires processes, not the client: the next call starts a fresh
	// child and is answered.
	if _, err := c.Active(ctx, "TASK-1"); err != nil {
		t.Fatalf("a call after the idle child was reaped: %v", err)
	}
	if again := idleChild(t, c); again == child {
		t.Fatal("the reaped child was handed out again")
	}
}

// TestAChildPastItsTTLIsNotReusedEvenBeforeTheReaperRuns holds take to the same
// bound as the timer. The reaper is stopped here so the only thing that can
// retire the expired child is the borrow itself.
func TestAChildPastItsTTLIsNotReusedEvenBeforeTheReaperRuns(t *testing.T) {
	c := client(t)
	c.pool.idleTTL = 300 * time.Millisecond
	seedTask(t, c, "TASK-1")
	ctx := context.Background()

	if _, err := c.Active(ctx, "TASK-1"); err != nil {
		t.Fatalf("active: %v", err)
	}
	stale := idleChild(t, c)
	c.pool.mu.Lock()
	if c.pool.reaper == nil {
		c.pool.mu.Unlock()
		t.Fatal("put parked a child without arming the reaper")
	}
	// Stopped but left non-nil, so armReaperLocked treats it as armed and
	// does not schedule another.
	c.pool.reaper.Stop()
	c.pool.mu.Unlock()

	time.Sleep(2 * c.pool.idleTTL)
	if processDone(stale) {
		t.Fatal("control: the stale child ended although the reaper was stopped")
	}
	if _, err := c.Active(ctx, "TASK-1"); err != nil {
		t.Fatalf("active after the TTL: %v", err)
	}
	if idleChild(t, c) == stale {
		t.Fatal("a child past its idle TTL was reused")
	}
	waitProcessDone(t, stale, "a child retired by take")
}

// TestReapingRacesBorrowingWithoutLosingACallOrASlot drives calls across the
// TTL boundary from several goroutines, so the reaper, take and put contend for
// the same children. Run under -race it is also the data-race check.
func TestReapingRacesBorrowingWithoutLosingACallOrASlot(t *testing.T) {
	c := client(t)
	c.pool.idleTTL = 15 * time.Millisecond
	seedTask(t, c, "TASK-1")
	ctx := context.Background()

	var wg sync.WaitGroup
	errs := make(chan error, 64)
	stop := time.Now().Add(1500 * time.Millisecond)
	for range 6 {
		wg.Go(func() {
			for time.Now().Before(stop) {
				if _, err := c.Active(ctx, "TASK-1"); err != nil {
					errs <- err
					return
				}
				time.Sleep(time.Duration(rand.IntN(40)) * time.Millisecond)
			}
		})
	}
	wg.Wait()
	close(errs)
	for err := range errs {
		t.Errorf("a call failed while the reaper ran: %v", err)
	}
	assertNoSlotsHeld(t, c)
}

// TestAChildReturnedAfterCloseIsEndedNotParked covers a call that finishes
// after its client was closed: the pool is terminal, so the child is ended
// rather than parked where no reaper will ever look.
func TestAChildReturnedAfterCloseIsEndedNotParked(t *testing.T) {
	c := New(binary(t), filepath.Join(t.TempDir(), "state.sqlite"))
	ctx := context.Background()
	child, err := c.pool.take(ctx, c.Binary, c.DB)
	if err != nil {
		t.Fatalf("take: %v", err)
	}
	c.Close()
	c.pool.put(child)

	if !processDone(child) {
		t.Fatal("a child returned after Close is still running")
	}
	c.pool.mu.Lock()
	idle, reaper := len(c.pool.idle), c.pool.reaper
	c.pool.mu.Unlock()
	if idle != 0 || reaper != nil {
		t.Fatalf("a closed pool parked a child (idle %d) or armed its reaper (%v)", idle, reaper != nil)
	}
	assertNoSlotsHeld(t, c)
}
