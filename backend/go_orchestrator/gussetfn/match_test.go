package gussetfn

import (
	"context"
	"errors"
	"sync"
	"testing"
	"time"

	"github.com/bharathvbcr/gusset"
)

func TestCheckAgreesAndDoesNotPoison(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	if err := Check(ctx); err != nil {
		t.Fatal(err)
	}
}

func TestNilContextIsRefused(t *testing.T) {
	_, err := Match(nil, "*.py", "a.py")
	if err == nil || err.Error() != "gusset: nil context" {
		t.Fatalf("got %v", err)
	}
}

func TestCancelledContextDoesNotMatch(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	_, err := Match(ctx, "*.py", "src/foo.py")
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("got %v", err)
	}
}

func TestOverlongFieldIsRefusedBeforeTheCall(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	pattern := make([]byte, maxField+1)
	for i := range pattern {
		pattern[i] = 'a'
	}
	_, err := Match(ctx, string(pattern), "a")
	if err == nil {
		t.Fatal("expected a length refusal")
	}
	if errors.Is(err, gusset.ErrPanic) {
		t.Fatalf("length refusal panicked: %v", err)
	}
}

func TestConcurrentMatches(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	var wg sync.WaitGroup
	errCh := make(chan error, 32)
	for g := 0; g < 32; g++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for i := 0; i < 20; i++ {
				got, err := Match(ctx, "*.py", "src/foo.py")
				if err != nil {
					errCh <- err
					return
				}
				if !got {
					errCh <- errors.New("concurrent match returned false")
					return
				}
			}
		}()
	}
	wg.Wait()
	close(errCh)
	for err := range errCh {
		t.Error(err)
	}
}

func TestMatchAny(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	patterns := []string{"*.rs", "*.py", "*.go"}
	matched, err := MatchAny(ctx, patterns, "src/foo.py")
	if err != nil {
		t.Fatalf("MatchAny: %v", err)
	}
	if !matched {
		t.Fatal("expected MatchAny to return true for src/foo.py")
	}

	matched, err = MatchAny(ctx, patterns, "src/foo.js")
	if err != nil {
		t.Fatalf("MatchAny: %v", err)
	}
	if matched {
		t.Fatal("expected MatchAny to return false for src/foo.js")
	}

	// Empty patterns
	matched, err = MatchAny(ctx, nil, "src/foo.py")
	if err != nil {
		t.Fatalf("MatchAny nil patterns: %v", err)
	}
	if matched {
		t.Fatal("expected MatchAny on nil patterns to return false")
	}
}
