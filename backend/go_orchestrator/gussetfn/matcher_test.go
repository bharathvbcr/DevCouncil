//go:build unix

package gussetfn

import (
	"strings"
	"testing"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/fnmatch"
)

// A question the engine cannot finish within the Matcher's bound gets
// fnmatch's answer, not an error: a busy engine is not a broken one, and an
// error here became a Hard engine_unavailable denial where fnmatch gave a
// Soft one.
func TestMatcherAnswersWithFnmatchWhenTheEngineIsSlow(t *testing.T) {
	slow := "*" + strings.Repeat("a", 2000) + "b"
	name := strings.Repeat("a", 16000)
	patterns := []string{slow, slow, slow, slow}
	m := Matcher{Timeout: time.Millisecond}
	before := Fallbacks()
	for _, fold := range []bool{false, true} {
		var got bool
		var err error
		want := fnmatch.MatchAny(patterns, name)
		if fold {
			got, err = m.MatchAnyFold(patterns, name)
			want = fnmatch.MatchAnyFold(patterns, name)
		} else {
			got, err = m.MatchAny(patterns, name)
		}
		if err != nil {
			t.Fatalf("fold=%v: a slow question failed instead of falling back: %v", fold, err)
		}
		if got != want {
			t.Fatalf("fold=%v: got %v, fnmatch %v", fold, got, want)
		}
	}
	if Fallbacks()-before < 2 {
		t.Fatalf("fallbacks %d -> %d: the engine answered within 1ms, so the fallback path was not exercised", before, Fallbacks())
	}
	// And the engine still answers an ordinary question within the default.
	if got, err := (Matcher{}).MatchAny([]string{"*.go"}, "a.go"); err != nil || !got {
		t.Fatalf("ordinary question = %v, %v", got, err)
	}
}
