//go:build unix

package gussetfn

import (
	"bytes"
	"context"
	"encoding/binary"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"testing"
	"time"

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
// name, case-sensitive and folded, including lists split over several frames.
func TestMatchAnyAgreesWithGoAcrossFrames(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()
	rows := parityRows(t)
	patterns := make([]string, 0, 3*len(rows))
	for i := 0; i < 3; i++ {
		for _, r := range rows {
			patterns = append(patterns, r[0])
		}
	}
	if _, n := encodeAny("x", patterns); n >= len(patterns) {
		t.Fatal("the full list fit one frame; the split path is not exercised")
	}
	for _, list := range [][]string{patterns[:1], patterns[:40], patterns} {
		for _, r := range rows[:200] {
			for _, name := range []string{r[1], strings.ToUpper(r[1])} {
				got, err := MatchAny(ctx, list, name)
				if err != nil {
					t.Fatalf("MatchAny(%d patterns, %q): %v", len(list), name, err)
				}
				if want := fnmatch.MatchAny(list, name); got != want {
					t.Errorf("MatchAny(%d patterns, %q) = %v, Go says %v", len(list), name, got, want)
				}
				got, err = MatchAnyFold(ctx, list, name)
				if err != nil {
					t.Fatalf("MatchAnyFold(%d patterns, %q): %v", len(list), name, err)
				}
				if want := fnmatch.MatchAnyFold(list, name); got != want {
					t.Errorf("MatchAnyFold(%d patterns, %q) = %v, Go says %v", len(list), name, got, want)
				}
			}
		}
	}
}

func TestEncodeAnyBoundsEachFrame(t *testing.T) {
	long := strings.Repeat("a", maxField/4)
	list := make([]string, 200)
	for i := range list {
		list[i] = long
	}
	for rest := list; len(rest) > 0; {
		frame, n := encodeAny(long, rest)
		if n == 0 {
			t.Fatal("encodeAny made no progress")
		}
		if n > 1 && len(frame) > maxFrameBytes {
			t.Fatalf("frame of %d bytes and %d patterns exceeds %d", len(frame), n, maxFrameBytes)
		}
		rest = rest[n:]
	}
	many := make([]string, maxPatterns+10)
	if _, n := encodeAny("", many); n != maxPatterns {
		t.Fatalf("frame carries %d patterns, cap is %d", n, maxPatterns)
	}
}

// Inputs past fnmatch's cap get fnmatch's answer, decided in Go: no match
// case-sensitively, a match folded. The old engine path made them an error,
// which a policy gate would have turned into a denial Go never gave.
func TestOversizedInputsGetGosAnswer(t *testing.T) {
	ctx := context.Background()
	over := strings.Repeat("a", 16_385)
	cases := []struct {
		patterns []string
		name     string
	}{
		{[]string{"*", over}, "a"},
		{[]string{over}, "a"},
		{[]string{"*"}, over},
		{[]string{"b*"}, strings.Repeat("é", 16_385)},
		{[]string{"*"}, strings.Repeat("a", 16_384)},
	}
	for _, c := range cases {
		got, err := MatchAny(ctx, c.patterns, c.name)
		if err != nil {
			t.Fatalf("MatchAny: %v", err)
		}
		if want := fnmatch.MatchAny(c.patterns, c.name); got != want {
			t.Errorf("MatchAny(%d patterns, %d-byte name) = %v, Go %v", len(c.patterns), len(c.name), got, want)
		}
		got, err = MatchAnyFold(ctx, c.patterns, c.name)
		if err != nil {
			t.Fatalf("MatchAnyFold: %v", err)
		}
		if want := fnmatch.MatchAnyFold(c.patterns, c.name); got != want {
			t.Errorf("MatchAnyFold(%d patterns, %d-byte name) = %v, Go %v", len(c.patterns), len(c.name), got, want)
		}
	}
	if _, err := MatchAny(nil, []string{"*"}, "a"); err == nil {
		t.Fatal("a nil context was accepted")
	}
	cctx, cancel := context.WithCancel(ctx)
	cancel()
	if _, err := MatchAny(cctx, nil, "a"); !errors.Is(err, context.Canceled) {
		t.Fatalf("empty list on a cancelled context = %v", err)
	}
	if _, err := MatchAny(cctx, []string{over}, "a"); !errors.Is(err, context.Canceled) {
		t.Fatalf("a Go-decided answer on a cancelled context = %v", err)
	}
}

// A frame over Gusset's 4 KiB inline copy travels in a Rust buffer and gets
// Go's answer: a long command or path is an ordinary policy input.
func TestLargeFramesTravelInABuffer(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	name := strings.Repeat("dir/", 3000) + "id_rsa"
	if fnmatch.Oversized(name) {
		t.Fatal("the name is past fnmatch's cap; Go would answer without a crossing")
	}
	if frame, _ := encodeAny(name, []string{"*id_rsa"}); len(frame) <= maxFrame {
		t.Fatalf("frame of %d bytes fits inline; the buffer path is not exercised", len(frame))
	}
	for _, p := range []string{"*id_rsa", "*.pem", "dir/*", "*/ID_RSA"} {
		got, err := Match(ctx, p, name)
		if err != nil {
			t.Fatalf("Match(%q): %v", p, err)
		}
		if want := fnmatch.Match(p, name); got != want {
			t.Errorf("Match(%q) = %v, Go %v", p, got, want)
		}
		got, err = MatchFold(ctx, p, name)
		if err != nil {
			t.Fatalf("MatchFold(%q): %v", p, err)
		}
		if want := fnmatch.MatchFold(p, name); got != want {
			t.Errorf("MatchFold(%q) = %v, Go %v", p, got, want)
		}
	}
}

// Invalid UTF-8 is matched the way fnmatch reads it, one U+FFFD per invalid
// byte. strings.ToValidUTF8 collapses a run into one, which would shift every
// '?' after it.
func TestInvalidUTF8GetsGosAnswer(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	for _, c := range [][2]string{
		{"???", "\xff\xfe\xfd"}, {"?", "\xff\xfe"}, {"*\xff", "a\xfe"}, {"[\xff]", "\xfe"},
		{"a?b", "a\xc3b"}, {"\xff*", "\xffX"},
	} {
		got, err := Match(ctx, c[0], c[1])
		if err != nil {
			t.Fatalf("Match(%q, %q): %v", c[0], c[1], err)
		}
		if want := fnmatch.Match(c[0], c[1]); got != want {
			t.Errorf("Match(%q, %q) = %v, Go %v", c[0], c[1], got, want)
		}
		got, err = MatchFold(ctx, c[0], c[1])
		if err != nil {
			t.Fatalf("MatchFold(%q, %q): %v", c[0], c[1], err)
		}
		if want := fnmatch.MatchFold(c[0], c[1]); got != want {
			t.Errorf("MatchFold(%q, %q) = %v, Go %v", c[0], c[1], got, want)
		}
	}
}

// rawAny builds a match-any frame byte for byte, so the engine's own
// validation is reachable: Go's encoder never sends invalid UTF-8.
func rawAny(name string, patterns ...string) []byte {
	buf := appendField(nil, name)
	buf = binary.LittleEndian.AppendUint32(buf, uint32(len(patterns)))
	for _, p := range patterns {
		buf = appendField(buf, p)
	}
	return buf
}

// A refused frame is an error, not a poison, and the answer cannot depend on
// order: a malformed pattern after a matching one is still refused, because
// the engine validates the whole frame before matching.
func TestEngineRefusesMalformedFramesWithoutPoison(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	actx := gusset.ContextWithOpcode(ctx, opcodeMatchAny)
	for name, frame := range map[string][]byte{
		"invalid UTF-8 after a match": rawAny("a.py", "*.py", "\xff"),
		"trailing bytes":              append(rawAny("a", "*"), 0),
		"truncated":                   rawAny("a", "*")[:9],
		"field past the cap":          binary.LittleEndian.AppendUint32(nil, maxField+1),
		"count past the cap":          binary.LittleEndian.AppendUint32(appendField(nil, "a"), maxPatterns+1),
	} {
		_, err := call(actx, frame)
		if err == nil {
			t.Errorf("%s: accepted", name)
			continue
		}
		if poisoned(err) {
			t.Fatalf("%s poisoned the handle: %v", name, err)
		}
	}
	if got, err := MatchAny(ctx, []string{"*.py"}, "a.py"); err != nil || !got {
		t.Fatalf("next MatchAny = %v, %v", got, err)
	}
}

func TestAnswerRejectsUnknownBytes(t *testing.T) {
	for _, out := range [][]byte{nil, {3}, {0, 0}, {0xff}} {
		if _, err := answer(out); err == nil {
			t.Errorf("answer(%v) accepted", out)
		}
	}
	for _, b := range []byte{noMatch, match, undecided} {
		if a, err := answer([]byte{b}); err != nil || a != b {
			t.Errorf("answer(%d) = %d, %v", b, a, err)
		}
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
	if prev != nil {
		defer prev.Close()
	}

	// The poison was a sibling's, so the call retires the handle and answers
	// from the replacement instead of returning ErrPoisoned.
	got, err := Match(ctx, "*.py", "a.py")
	if err != nil || !got {
		t.Fatalf("Match on the poisoned handle = %v, %v; want a retried match", got, err)
	}
	if h := current.Load(); h == bad || h == nil {
		t.Fatal("the poisoned handle was not replaced")
	}
}

// This frame's own panic is not retried: the retry would poison the
// replacement with the same input.
func TestOwnPanicIsNotRetried(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	before := opens.Load()
	_, err := call(gusset.ContextWithOpcode(ctx, opcodeSelfTestPanic), nil)
	if !errors.Is(err, gusset.ErrPanic) {
		t.Fatalf("call = %v, want ErrPanic", err)
	}
	if n := opens.Load() - before; n > 1 {
		t.Fatalf("%d handles opened for one panicking frame", n)
	}
	if got, err := Match(ctx, "*.py", "a.py"); err != nil || !got {
		t.Fatalf("Match after a panicking frame = %v, %v", got, err)
	}
}

// Many goroutines hit a handle that becomes poisoned under them. Every call
// must still succeed — the poison was another call's, and call retries once
// on the replacement — and exactly one replacement is opened per poisoning.
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
		before := opens.Load()
		var wg sync.WaitGroup
		errs := make(chan error, 64*10)
		for g := 0; g < 64; g++ {
			wg.Add(1)
			go func() {
				defer wg.Done()
				for i := 0; i < 10; i++ {
					got, err := Match(ctx, "*.py", "a.py")
					if err != nil {
						errs <- err
						continue
					}
					if !got {
						errs <- errors.New("match returned false")
					}
				}
			}()
		}
		wg.Wait()
		close(errs)
		for err := range errs {
			t.Errorf("round %d: %v", round, err)
		}
		if n := opens.Load() - before; n != 1 {
			t.Fatalf("round %d: %d handles opened to replace one poisoned handle", round, n)
		}
	}
}

// An opcode already on the caller's context must not reach the frame: opcode
// 0 would decode a match-any frame as a single match, the self-test opcode
// panicked the shared handle.
func TestMatchIgnoresAnOpcodeOnTheContext(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	for _, op := range []uint32{opcodeMatch, opcodeSelfTestPanic} {
		got, err := Match(gusset.ContextWithOpcode(ctx, op), "*.py", "a.py")
		if err != nil || !got {
			t.Fatalf("Match under opcode %#x = %v, %v", op, got, err)
		}
	}
	if h := current.Load(); h == nil {
		t.Fatal("no shared handle")
	}
	if got, err := Match(ctx, "*.py", "a.py"); err != nil || !got {
		t.Fatalf("shared handle after opcode contexts: %v, %v", got, err)
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

// DrainLogs terminates on an idle ring and reports the bytes it wrote. It
// does not prove anything was logged: gusset's ring receives no line for a
// caught panic (that goes to FfiStatus), so there is nothing reliable to
// drain here.
func TestDrainLogsTerminatesAndCounts(t *testing.T) {
	var buf bytes.Buffer
	if _, err := DrainLogs(&buf); err != nil {
		t.Fatal(err)
	}
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

// FuzzMatchParity is the differential check across the boundary: for every
// input, valid UTF-8 or not and of any length, the engine path answers what
// fnmatch answers, case-sensitive and folded, and never errors. Run with
// go test -fuzz=FuzzMatchParity ./gussetfn.
func FuzzMatchParity(f *testing.F) {
	for _, seed := range [][2]string{
		{"*.py", "src/foo.py"}, {"[!a-c]*", "d"}, {"[c-a-e]", "e"}, {"[]]", "]"},
		{"[!]", "!"}, {"**?*", "ab"}, {"\xff", "a"}, {"[\\]", "\\"}, {"a*b*c*d*e", "aXbXcXdXe"},
		{"?\xff?", "\xfe\xfe\xfe"}, {"*.PEM", "a.pem"}, {"İ*", "i̇x"}, {"[Σ]", "ς"},
	} {
		f.Add(seed[0], seed[1])
	}
	ctx := context.Background()
	f.Fuzz(func(t *testing.T, pattern, name string) {
		got, err := Match(ctx, pattern, name)
		if err != nil {
			t.Fatalf("Match(%q, %q): %v", pattern, name, err)
		}
		if want := fnmatch.Match(pattern, name); got != want {
			t.Fatalf("Match(%q, %q): gusset %v, Go %v", pattern, name, got, want)
		}
		got, err = MatchFold(ctx, pattern, name)
		if err != nil {
			t.Fatalf("MatchFold(%q, %q): %v", pattern, name, err)
		}
		if want := fnmatch.MatchFold(pattern, name); got != want {
			t.Fatalf("MatchFold(%q, %q): gusset %v, Go %v", pattern, name, got, want)
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

// The umbrella installs gusset's Counting allocator. Gusset counts its own
// buffers by hand whether or not Counting is installed, so buffer bytes prove
// nothing about the allocator; AllocCount across plain Matches does — each
// frame decode allocates a Vec that only a global Counting sees.
func TestCountingIsTheGlobalAllocator(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	if _, err := Match(ctx, "*", "a"); err != nil {
		t.Fatal(err)
	}
	before := gusset.Stats().AllocCount
	for i := 0; i < 50; i++ {
		if _, err := Match(ctx, "*.py", "src/foo.py"); err != nil {
			t.Fatal(err)
		}
	}
	if after := gusset.Stats().AllocCount; after < before+50 {
		t.Fatalf("AllocCount %d -> %d over 50 Matches: Counting is not the global allocator", before, after)
	}
}

// A Rust buffer is counted once, not by both gusset's buffer accounting and
// the global allocator.
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
	// Process-wide, so a concurrent free from an earlier test's cleanup can
	// pull it slightly under n; double-counting would put it near 2n.
	if delta < n-n/16 || delta > n+n/2 {
		t.Fatalf("an %d-byte buffer moved LiveBytes by %d; want about %d once", n, delta, n)
	}
	if int64(after)-int64(before) > n/8 {
		t.Fatalf("LiveBytes %d after Free, %d before", after, before)
	}
}

// Calls racing Close return a result or ErrClosed, never an unclassified
// "handle is closed". Named ZZ and reset by defer because Close is
// process-wide; Go runs tests in source order, so this stays after the rest
// of this file (match_test.go runs later and relies on the reset).
func TestZZCloseRacingCallsSeeErrClosed(t *testing.T) {
	defer func() { closed.Store(false) }()
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	if _, err := engine(); err != nil {
		t.Fatal(err)
	}
	var wg sync.WaitGroup
	errs := make(chan error, 32*50)
	for g := 0; g < 32; g++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for i := 0; i < 50; i++ {
				if _, err := Match(ctx, "*.py", "a.py"); err != nil && !errors.Is(err, ErrClosed) {
					errs <- err
				}
			}
		}()
	}
	time.Sleep(2 * time.Millisecond)
	if err := Close(); err != nil {
		t.Errorf("Close: %v", err)
	}
	wg.Wait()
	close(errs)
	for err := range errs {
		t.Errorf("call racing Close: %v", err)
	}
	if _, err := Match(context.Background(), "*", "a"); !errors.Is(err, ErrClosed) {
		t.Fatalf("Match after Close = %v, want ErrClosed", err)
	}
	if err := Close(); err != nil {
		t.Fatalf("second Close: %v", err)
	}
}

// Shutdown is process-wide and one-way, so it runs in a child process.
func TestShutdownIsBoundedAndRefusesLaterCalls(t *testing.T) {
	if os.Getenv("GUSSETFN_SHUTDOWN_CHILD") == "1" {
		ctx := context.Background()
		if _, err := Match(ctx, "*", "a"); err != nil {
			fmt.Println("pre-shutdown match:", err)
			os.Exit(3)
		}
		if err := Shutdown(time.Second); err != nil {
			fmt.Println("shutdown:", err)
			os.Exit(4)
		}
		if _, err := Match(ctx, "*", "a"); !errors.Is(err, ErrClosed) {
			fmt.Println("post-shutdown match:", err)
			os.Exit(5)
		}
		os.Exit(0)
	}
	cmd := exec.Command(os.Args[0], "-test.run=^TestShutdownIsBoundedAndRefusesLaterCalls$")
	cmd.Env = append(os.Environ(), "GUSSETFN_SHUTDOWN_CHILD=1")
	out, err := cmd.CombinedOutput()
	if err != nil {
		t.Fatalf("child: %v\n%s", err, out)
	}
}

// Large frames travel in Rust-owned buffers, and a sibling's panic retires
// the handle that owns them. Every call must still come back with the right
// answer: a copy racing the retire used to touch freed Rust memory, and a
// result buffer freed under the call came back as a malformed answer
// instead of ErrClosed, so it was denied rather than retried.
func TestLargeFramesSurviveRetiresUnderThem(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()
	name := strings.Repeat("dir/", 3000) + "id_rsa"
	want := fnmatch.Match("*id_rsa", name)
	stop := make(chan struct{})
	var poisoner sync.WaitGroup
	poisoner.Add(1)
	go func() {
		defer poisoner.Done()
		for {
			select {
			case <-stop:
				return
			default:
			}
			if _, err := engine(); err != nil {
				continue
			}
			bad, err := gusset.Open(gusset.WithPoolSize(1), gusset.WithOpcode(opcodeSelfTestPanic))
			if err != nil {
				continue
			}
			_, _ = bad.Call(ctx, nil)
			if prev := current.Swap(bad); prev != nil {
				go closeHandle(prev)
			}
			time.Sleep(2 * time.Millisecond)
		}
	}()
	var wg sync.WaitGroup
	errs := make(chan error, 16*25)
	for g := 0; g < 16; g++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for i := 0; i < 25; i++ {
				got, err := Match(ctx, "*id_rsa", name)
				switch {
				case err != nil && !poisoned(err) && !errors.Is(err, gusset.ErrClosed):
					errs <- err
				case err == nil && got != want:
					errs <- fmt.Errorf("got %v, want %v", got, want)
				}
			}
		}()
	}
	wg.Wait()
	close(stop)
	poisoner.Wait()
	close(errs)
	for err := range errs {
		t.Error(err)
	}
	if got, err := Match(ctx, "*id_rsa", name); err != nil || got != want {
		t.Fatalf("after the storm: %v, %v", got, err)
	}
}
