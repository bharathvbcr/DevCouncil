package gussetfn

import (
	"bytes"
	"context"
	"errors"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"testing"
	"time"
	"unicode/utf8"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/fnmatch"
	"github.com/bharathvbcr/gusset"
)

// parityRows loads the CPython fnmatch table dc-glob and fnmatch both pin.
func parityRows(t testing.TB) [][3]string {
	t.Helper()
	_, file, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("cannot locate boundary_test.go")
	}
	path := filepath.Join(filepath.Dir(file), "..", "..", "..", "testdata", "fnmatch-parity.tsv")
	raw, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read parity fixture: %v", err)
	}
	var rows [][3]string
	for _, line := range strings.Split(string(raw), "\n") {
		if strings.HasPrefix(line, "#") || strings.TrimSpace(line) == "" {
			continue
		}
		cols := strings.Split(line, "\t")
		if len(cols) != 3 {
			t.Fatalf("malformed fixture row %q", line)
		}
		rows = append(rows, [3]string{cols[0], cols[1], cols[2]})
	}
	if len(rows) < 800 {
		t.Fatalf("only %d fixture rows loaded", len(rows))
	}
	return rows
}

// The CPython table, through the boundary. dc-glob's own test reads it
// in-process; this reads it through cgo, the ring and the frame codec, which
// is the path a host actually takes.
func TestParityFixtureAcrossTheBoundary(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	for _, row := range parityRows(t) {
		pattern, name, want := row[0], row[1], row[2] == "true"
		got, err := Match(ctx, pattern, name)
		if err != nil {
			t.Fatalf("Match(%q, %q): %v", pattern, name, err)
		}
		if got != want {
			t.Errorf("Match(%q, %q) = %v across the boundary, CPython says %v", pattern, name, got, want)
		}
		if goGot := fnmatch.Match(pattern, name); goGot != got {
			t.Errorf("gusset %v and Go fnmatch %v disagree on (%q, %q)", got, goGot, pattern, name)
		}
	}
}

// MatchAny over the fixture's patterns agrees with the Go reference for each
// name, including lists that must be split over several frames.
func TestMatchAnyAgreesWithGoAcrossFrames(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	rows := parityRows(t)
	patterns := make([]string, 0, len(rows))
	for _, r := range rows {
		patterns = append(patterns, r[0])
	}
	for _, list := range [][]string{patterns[:1], patterns[:40], patterns} {
		if _, n := encodeAny("x", list); len(list) == len(patterns) && n >= len(list) {
			t.Fatal("the full list fit one frame; the split path is not exercised")
		}
		for _, r := range rows[:200] {
			got, err := MatchAny(ctx, list, r[1])
			if err != nil {
				t.Fatalf("MatchAny(%d patterns, %q): %v", len(list), r[1], err)
			}
			if want := fnmatch.MatchAny(list, r[1]); got != want {
				t.Errorf("MatchAny(%d patterns, %q) = %v, Go says %v", len(list), r[1], got, want)
			}
		}
	}
}

func TestEncodeAnyStaysUnderTheInlineLimit(t *testing.T) {
	long := strings.Repeat("a", maxField)
	list := []string{long, long, long, long, long}
	name := strings.Repeat("b", maxField)
	for rest := list; len(rest) > 0; {
		frame, n := encodeAny(name, rest)
		if n == 0 {
			t.Fatal("encodeAny made no progress")
		}
		if len(frame) > maxFrame {
			t.Fatalf("frame of %d bytes exceeds %d", len(frame), maxFrame)
		}
		rest = rest[n:]
	}
	many := make([]string, maxPatterns+10)
	if _, n := encodeAny("", many); n > maxPatterns {
		t.Fatalf("frame carries %d patterns, cap is %d", n, maxPatterns)
	}
}

func TestMatchAnyRefusesBeforeTheCall(t *testing.T) {
	ctx := context.Background()
	long := strings.Repeat("a", maxField+1)
	if _, err := MatchAny(ctx, []string{"*", long}, "a"); err == nil {
		t.Fatal("an overlong pattern late in the list was accepted")
	}
	if _, err := MatchAny(ctx, []string{"*"}, long); err == nil {
		t.Fatal("an overlong name was accepted")
	}
	if _, err := MatchAny(nil, []string{"*"}, "a"); err == nil {
		t.Fatal("a nil context was accepted")
	}
	cctx, cancel := context.WithCancel(ctx)
	cancel()
	if _, err := MatchAny(cctx, nil, "a"); !errors.Is(err, context.Canceled) {
		t.Fatalf("empty list on a cancelled context = %v", err)
	}
}

// A malformed pattern after a matching one is still an error: the engine
// validates the frame before matching, so the answer cannot depend on order.
func TestMatchAnyInvalidUTF8IsAnErrorNotAPoison(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	_, err := MatchAny(ctx, []string{"*.py", "\xff"}, "a.py")
	if err == nil || !strings.Contains(err.Error(), "UTF-8") {
		t.Fatalf("got %v, want a UTF-8 error", err)
	}
	if poisoned(err) {
		t.Fatalf("invalid UTF-8 poisoned the handle: %v", err)
	}
	if got, err := MatchAny(ctx, []string{"*.py"}, "a.py"); err != nil || !got {
		t.Fatalf("next MatchAny = %v, %v", got, err)
	}
}

// A poisoned shared handle is retired and the next call opens a new one.
// Before, the handle was held by a sync.Once and every later call failed.
func TestPoisonedHandleIsReplaced(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	if _, err := engine(); err != nil {
		t.Fatal(err)
	}
	bad, err := gusset.Open(gusset.WithPoolSize(1), gusset.WithOpcode(opcodeSelfTestPanic))
	if err != nil {
		t.Fatal(err)
	}
	if _, err := bad.Call(ctx, nil); !errors.Is(err, gusset.ErrPanic) {
		t.Fatalf("self-test panic = %v", err)
	}
	prev := current.Swap(bad)

	if _, err := Match(ctx, "*.py", "a.py"); !errors.Is(err, gusset.ErrPoisoned) {
		t.Fatalf("Match on the poisoned handle = %v, want ErrPoisoned", err)
	}
	if current.Load() == bad {
		t.Fatal("the poisoned handle is still the shared handle")
	}
	got, err := Match(ctx, "*.py", "a.py")
	if err != nil || !got {
		t.Fatalf("Match after replacement = %v, %v", got, err)
	}
	if prev != nil {
		_ = prev.Close()
	}
}

// Many goroutines hit a handle that becomes poisoned under them. Every call
// returns (no hang), every error is a Gusset error, and exactly one new
// handle serves afterwards.
func TestPoisonUnderConcurrencyRecovers(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	for round := 0; round < 5; round++ {
		if _, err := engine(); err != nil {
			t.Fatal(err)
		}
		bad, err := gusset.Open(gusset.WithPoolSize(1), gusset.WithOpcode(opcodeSelfTestPanic))
		if err != nil {
			t.Fatal(err)
		}
		_, _ = bad.Call(ctx, nil)
		if prev := current.Swap(bad); prev != nil {
			_ = prev.Close()
		}
		var wg sync.WaitGroup
		errs := make(chan error, 64)
		for g := 0; g < 64; g++ {
			wg.Add(1)
			go func() {
				defer wg.Done()
				for i := 0; i < 10; i++ {
					got, err := Match(ctx, "*.py", "a.py")
					if err == nil && !got {
						errs <- errors.New("match returned false")
						return
					}
					if err != nil && ctx.Err() != nil {
						errs <- err
						return
					}
				}
			}()
		}
		wg.Wait()
		close(errs)
		for err := range errs {
			t.Error(err)
		}
		if got, err := Match(ctx, "*.py", "a.py"); err != nil || !got {
			t.Fatalf("round %d: settled Match = %v, %v", round, got, err)
		}
	}
}

func TestCancellationDoesNotPoison(t *testing.T) {
	for i := 0; i < 200; i++ {
		ctx, cancel := context.WithTimeout(context.Background(), time.Duration(i%7)*time.Microsecond)
		_, err := MatchAny(ctx, []string{"*.rs", "*.py"}, "a.py")
		cancel()
		if err != nil && poisoned(err) {
			t.Fatalf("iteration %d: a deadline poisoned the handle: %v", i, err)
		}
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	if got, err := Match(ctx, "*.py", "a.py"); err != nil || !got {
		t.Fatalf("Match after cancellations = %v, %v", got, err)
	}
}

func TestDrainLogsWritesWhatRustLogged(t *testing.T) {
	var buf bytes.Buffer
	if _, err := DrainLogs(&buf); err != nil {
		t.Fatal(err)
	}
	// A second drain of an idle ring is empty; the call must not block.
	buf.Reset()
	n, err := DrainLogs(&buf)
	if err != nil || n != buf.Len() {
		t.Fatalf("DrainLogs = %d, %v with %d bytes written", n, err, buf.Len())
	}
}

func TestSelfTestProvesTheFirewall(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	if err := SelfTest(ctx); err != nil {
		t.Fatal(err)
	}
}

// FuzzMatchParity is the differential check across the boundary: for valid
// UTF-8 the Rust engine and the Go matcher must agree, and anything else is
// an engine error that leaves the handle usable. Run with
// go test -fuzz=FuzzMatchParity ./gussetfn.
func FuzzMatchParity(f *testing.F) {
	for _, seed := range [][2]string{
		{"*.py", "src/foo.py"}, {"[!a-c]*", "d"}, {"[c-a-e]", "e"}, {"[]]", "]"},
		{"[!]", "!"}, {"**?*", "ab"}, {"\xff", "a"}, {"[\\]", "\\"}, {"a*b*c*d*e", "aXbXcXdXe"},
	} {
		f.Add(seed[0], seed[1])
	}
	ctx := context.Background()
	f.Fuzz(func(t *testing.T, pattern, name string) {
		if len(pattern) > maxField || len(name) > maxField {
			if _, err := Match(ctx, pattern, name); err == nil {
				t.Fatal("an overlong field was accepted")
			}
			return
		}
		got, err := Match(ctx, pattern, name)
		if !utf8.ValidString(pattern) || !utf8.ValidString(name) {
			if err == nil {
				t.Fatalf("invalid UTF-8 (%q, %q) matched instead of failing", pattern, name)
			}
			if poisoned(err) {
				t.Fatalf("invalid UTF-8 poisoned the handle: %v", err)
			}
			return
		}
		if err != nil {
			t.Fatalf("Match(%q, %q): %v", pattern, name, err)
		}
		if want := fnmatch.Match(pattern, name); got != want {
			t.Fatalf("Match(%q, %q): gusset %v, Go %v", pattern, name, got, want)
		}
		any, err := MatchAny(ctx, []string{pattern, pattern + "x"}, name)
		if err != nil {
			t.Fatalf("MatchAny: %v", err)
		}
		if want := fnmatch.MatchAny([]string{pattern, pattern + "x"}, name); any != want {
			t.Fatalf("MatchAny(%q, %q): gusset %v, Go %v", pattern, name, any, want)
		}
	})
}

// Close is last: it shuts the process-wide engine. reopen restores it so a
// -count>1 run still has an engine.
func TestZZCloseRefusesLaterCalls(t *testing.T) {
	defer func() { closed.Store(false) }()
	if _, err := engine(); err != nil {
		t.Fatal(err)
	}
	if err := Close(); err != nil {
		t.Fatalf("Close: %v", err)
	}
	if _, err := Match(context.Background(), "*", "a"); !errors.Is(err, ErrClosed) {
		t.Fatalf("Match after Close = %v, want ErrClosed", err)
	}
	if err := Close(); err != nil {
		t.Fatalf("second Close: %v", err)
	}
}

// The umbrella installs gusset's Counting allocator. A Rust buffer is then
// counted once — by Gusset's own buffer accounting or by the global
// allocator, not both — and it is counted at all: before, Stats read zero.
func TestStatsCountBuffersOnce(t *testing.T) {
	if _, err := engine(); err != nil {
		t.Fatal(err)
	}
	h, err := gusset.Open(gusset.WithPoolSize(1))
	if err != nil {
		t.Fatal(err)
	}
	defer h.Close()
	const n = 8 << 20
	before := gusset.Stats().LiveBytes
	buf, err := h.NewBuffer(n)
	if err != nil {
		t.Fatal(err)
	}
	during := gusset.Stats().LiveBytes
	if err := buf.Free(); err != nil {
		t.Fatal(err)
	}
	after := gusset.Stats().LiveBytes
	delta := int64(during) - int64(before)
	// Other goroutines' Rust allocations are small next to 8 MiB.
	if delta < n || delta > n+n/2 {
		t.Fatalf("an %d-byte buffer moved LiveBytes by %d; want about %d once", n, delta, n)
	}
	if int64(after)-int64(before) > n/8 {
		t.Fatalf("LiveBytes %d after Free, %d before", after, before)
	}
}
