//go:build unix

package gussetfn

import (
	"context"
	"fmt"
	"strings"
	"testing"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/fnmatch"
)

// Every answer in a batch is fnmatch's, over the fixture's patterns in lists
// of the sizes the policy gates ask (one planned file, the 19 secret and 24
// protected patterns), mixed case-sensitive and folded, with the inputs Go
// settles itself: oversized patterns and names, empty lists, invalid UTF-8.
func TestMatchBatchAgreesWithFnmatch(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()
	rows := parityRows(t)
	over := strings.Repeat("x", 16_385)
	var qs []Query
	for _, size := range []int{1, 3, 19, 24} {
		for start := 0; start+size <= len(rows) && start < 240; start += size {
			var list []string
			for _, r := range rows[start : start+size] {
				list = append(list, r[0])
			}
			qs = append(qs, Query{Patterns: list}, Query{Patterns: list, Fold: true})
		}
	}
	qs = append(qs,
		Query{}, Query{Fold: true},
		Query{Patterns: []string{over, "*.go"}}, Query{Patterns: []string{over, "*.go"}, Fold: true},
		Query{Patterns: []string{over}}, Query{Patterns: []string{"?\xff?", "[!a]"}},
	)
	names := []string{"\xff\xff\xff", "\xff", "SRC/A.GO", over, strings.Repeat("dir/", 3000) + "Id_Rsa"}
	for _, r := range rows[:120] {
		names = append(names, r[1], strings.ToUpper(r[1]))
	}
	checked := 0
	for _, name := range names {
		got, err := MatchBatch(ctx, name, qs)
		if err != nil {
			t.Fatalf("MatchBatch(%.40q): %v", name, err)
		}
		for i, q := range qs {
			want := fnmatch.MatchAny(q.Patterns, name)
			if q.Fold {
				want = fnmatch.MatchAnyFold(q.Patterns, name)
			}
			if got[i] != want {
				t.Errorf("name %.40q fold=%v patterns %.80q: engine %v, fnmatch %v", name, q.Fold, q.Patterns, got[i], want)
			}
			checked++
		}
	}
	if checked < 20_000 {
		t.Fatalf("only %d answers compared", checked)
	}
}

// Once its lists are prepared, a batch is one crossing, whatever its size.
func TestMatchBatchOfPreparedListsIsOneCrossing(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	var qs []Query
	for i := 0; i < 12; i++ {
		qs = append(qs, Query{Patterns: []string{fmt.Sprintf("*.%d", i), "src/*"}, Fold: i%2 == 0})
	}
	if _, err := MatchBatch(ctx, "src/a.go", qs); err != nil {
		t.Fatal(err)
	}
	before := crossings.Load()
	got, err := MatchBatch(ctx, "docs/a.7", qs)
	if err != nil {
		t.Fatal(err)
	}
	if n := crossings.Load() - before; n != 1 {
		t.Fatalf("a batch of %d prepared lists took %d crossings, want 1", len(qs), n)
	}
	for i, q := range qs {
		if want := q.Patterns[0] == "*.7"; got[i] != want {
			t.Errorf("list %d = %v, want %v", i, got[i], want)
		}
	}
}

// The prepared list is found by its patterns, never by the slice holding
// them: a backing array reused for another list must not answer for the
// one it used to hold.
func TestPreparedListsAreKeyedOnContent(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	list := []string{"*.go"}
	if got, err := MatchAny(ctx, list, "a.go"); err != nil || !got {
		t.Fatalf("*.go on a.go = %v, %v", got, err)
	}
	list[0] = "*.rs"
	if got, err := MatchAny(ctx, list, "a.go"); err != nil || got {
		t.Fatalf("the reused slice now holds *.rs, and a.go matched it: %v, %v", got, err)
	}
	if got, err := MatchAnyFold(ctx, list, "A.RS"); err != nil || !got {
		t.Fatalf("folded *.rs on A.RS = %v, %v", got, err)
	}
}

// A full registry is slower, never wrong and never a refusal: a list it
// cannot hold is asked with match-any frames. It fills the process's
// registry, so it runs last in this package.
func TestZZZFullRegistryStillAnswers(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()
	var last *list
	for i := 0; i <= maxPrepared; i++ {
		l, err := lookup(ctx, []string{fmt.Sprintf("fill-%d-*", i)}, false)
		if err != nil {
			t.Fatalf("list %d: %v", i, err)
		}
		last = l
	}
	if last.prepared {
		t.Fatalf("list %d was prepared; the registry never filled", maxPrepared)
	}
	for _, c := range []struct {
		patterns []string
		name     string
		fold     bool
	}{
		{[]string{"after-full/*"}, "after-full/a", false},
		{[]string{"after-full/*"}, "AFTER-FULL/a", true},
		{[]string{"after-full/*"}, "AFTER-FULL/a", false},
	} {
		got, err := ask(ctx, c.patterns, c.name, c.fold)
		if err != nil {
			t.Fatalf("%v on %q: %v", c.patterns, c.name, err)
		}
		want := fnmatch.MatchAny(c.patterns, c.name)
		if c.fold {
			want = fnmatch.MatchAnyFold(c.patterns, c.name)
		}
		if got != want {
			t.Errorf("%v on %q fold=%v: %v, fnmatch %v", c.patterns, c.name, c.fold, got, want)
		}
	}
}
