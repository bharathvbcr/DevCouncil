package safefile

import (
	"errors"
	"os"
	"path/filepath"
	"testing"
)

// Anything that is not one of the platform's passing refusals is the answer:
// one attempt, and that error.
func TestRetryTransientReturnsAnOrdinaryErrorAtOnce(t *testing.T) {
	calls := 0
	real := errors.New("permission denied for good")
	err := RetryTransient(func() error { calls++; return real })
	if !errors.Is(err, real) || calls != 1 {
		t.Fatalf("err = %v after %d calls, want the error after exactly 1", err, calls)
	}
}

// What RetryTransient retries is the platform's own list: a sharing violation
// on Windows waits, and elsewhere there is no such refusal to wait for.
func TestRetryTransientWaitsOnlyForThePlatformsPassingRefusals(t *testing.T) {
	calls := 0
	err := RetryTransient(func() error {
		calls++
		if calls < 3 {
			return errorSharingViolationForTest
		}
		return nil
	})
	if transientRename(errorSharingViolationForTest) {
		if err != nil || calls != 3 {
			t.Fatalf("err = %v after %d calls, want success on the 3rd", err, calls)
		}
		return
	}
	if err == nil || calls != 1 {
		t.Fatalf("err = %v after %d calls; off Windows nothing is retried", err, calls)
	}
}

func TestReadFileReadsAndReportsAMissingFileAsMissing(t *testing.T) {
	path := filepath.Join(t.TempDir(), "f")
	if _, err := ReadFile(path); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("missing file: err = %v, want os.ErrNotExist", err)
	}
	if err := os.WriteFile(path, []byte("whole"), 0o644); err != nil {
		t.Fatal(err)
	}
	if got, err := ReadFile(path); err != nil || string(got) != "whole" {
		t.Fatalf("got %q, %v", got, err)
	}
}
