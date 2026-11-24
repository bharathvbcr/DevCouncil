package safefile

import (
	"errors"
	"testing"
	"time"
)

var errBusy = errors.New("busy: another handle has the destination open")

func TestRenameWaitsOutATransientRefusalAndThenSucceeds(t *testing.T) {
	calls := 0
	rename := func(old, new string) error {
		calls++
		if calls < 4 {
			return errBusy
		}
		return nil
	}
	err := renameWithRetry(rename, func(e error) bool { return errors.Is(e, errBusy) }, "a", "b", time.Second)
	if err != nil || calls != 4 {
		t.Fatalf("err = %v after %d calls, want success on the 4th", err, calls)
	}
}

// A refusal that is not transient must not be retried: a retry would only delay
// the same answer, and for a real permission error it would hide nothing but
// cost the caller its time.
func TestRenameReturnsANonTransientRefusalAtOnce(t *testing.T) {
	calls := 0
	real := errors.New("permission denied")
	err := renameWithRetry(func(string, string) error { calls++; return real }, func(e error) bool { return errors.Is(e, errBusy) }, "a", "b", time.Second)
	if !errors.Is(err, real) || calls != 1 {
		t.Fatalf("err = %v after %d calls, want the refusal after exactly 1", err, calls)
	}
}

// A transient refusal that never clears is still a failure, carrying the
// refusal itself, and the wait ends at the budget.
func TestRenameGivesUpAtTheBudgetWithTheLastRefusal(t *testing.T) {
	start := time.Now()
	err := renameWithRetry(func(string, string) error { return errBusy }, func(error) bool { return true }, "a", "b", 60*time.Millisecond)
	if !errors.Is(err, errBusy) {
		t.Fatalf("err = %v, want the refusal", err)
	}
	if waited := time.Since(start); waited < 40*time.Millisecond || waited > 2*time.Second {
		t.Fatalf("waited %s for a 60ms budget", waited)
	}
}

func TestRenameWithZeroBudgetTriesOnce(t *testing.T) {
	calls := 0
	_ = renameWithRetry(func(string, string) error { calls++; return errBusy }, func(error) bool { return true }, "a", "b", 0)
	if calls != 1 {
		t.Fatalf("%d attempts under a zero budget, want 1", calls)
	}
}
