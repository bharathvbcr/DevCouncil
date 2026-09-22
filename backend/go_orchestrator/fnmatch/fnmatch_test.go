package fnmatch

import (
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"testing"
	"time"
)

// fixturePath is the one CPython-generated parity table, resolved from this
// source file so the test does not depend on the process working directory.
// rust/dc-glob include_str!s the same file. Regenerate with
// scripts/gen-fnmatch-parity.py.
func fixturePath(t *testing.T) string {
	t.Helper()
	_, file, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("cannot locate fnmatch_test.go")
	}
	return filepath.Join(filepath.Dir(file), "..", "..", "..", "testdata", "fnmatch-parity.tsv")
}

// TestMatchesPythonFnmatch is the cross-language parity gate.
func TestMatchesPythonFnmatch(t *testing.T) {
	path := fixturePath(t)
	raw, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read parity fixture %s: %v", path, err)
	}
	checked := 0
	for i, line := range strings.Split(string(raw), "\n") {
		if strings.HasPrefix(line, "#") || strings.TrimSpace(line) == "" {
			continue
		}
		cols := strings.Split(line, "\t")
		if len(cols) != 3 {
			t.Fatalf("line %d is malformed: %q", i+1, line)
		}
		pattern, name, want := cols[0], cols[1], cols[2] == "true"
		if got := Match(pattern, name); got != want {
			t.Errorf("Match(%q, %q) = %v, want %v (CPython, line %d)", pattern, name, got, want, i+1)
		}
		checked++
	}
	// A fixture that failed to load would make this test pass by checking nothing.
	if !strings.Contains(string(raw), "[c-a-e]\te\ttrue\n") {
		t.Fatal("parity fixture is missing the inverted-range row; the two matchers can drift again")
	}
	if checked < 800 {
		t.Fatalf("only %d cases loaded from %s", checked, path)
	}
}

// TestStarCrossesSeparator pins the single behaviour that separates this
// package from path.Match. If this ever flips, every secret and restricted
// path rule silently stops matching nested paths.
func TestStarCrossesSeparator(t *testing.T) {
	if !Match("*.py", "src/deep/foo.py") {
		t.Fatal(`Match("*.py", "src/deep/foo.py") = false; Python says true`)
	}
	if !Match(".claude/*", ".claude/agents/x.md") {
		t.Fatal(`".claude/*" must match nested paths, or the restricted-path rule leaks`)
	}
}

// TestDoubleStarRequiresASeparator is the other half: "**/.env" deliberately
// does not match a bare ".env", which is why DevCouncil lists both patterns.
func TestDoubleStarRequiresASeparator(t *testing.T) {
	if Match("**/.env", ".env") {
		t.Fatal(`Match("**/.env", ".env") = true; Python says false`)
	}
	if !Match("**/.env", "a/b/.env") {
		t.Fatal(`Match("**/.env", "a/b/.env") = false; Python says true`)
	}
}

func TestUnterminatedClassNeverMatchesEverything(t *testing.T) {
	// A malformed pattern must not become a wildcard.
	if Match("a[bc", "anything at all") {
		t.Fatal("unterminated character class matched an unrelated string")
	}
	if !Match("a[bc", "a[bc") {
		t.Fatal("unterminated '[' should be treated as a literal")
	}
}

func TestMatchAny(t *testing.T) {
	pats := []string{"*.md", "**/*.pem"}
	if !MatchAny(pats, "deep/key.pem") {
		t.Fatal("MatchAny missed a matching pattern")
	}
	if MatchAny(pats, "src/main.go") {
		t.Fatal("MatchAny matched an unrelated path")
	}
}

// TestCPythonNormalizesInvertedRanges is the case the regexp translation got
// wrong. CPython drops an out-of-order range and keeps the characters that
// remain, so these patterns still match. A translator that asks RE2 to compile
// `[c-a-e]` fails the whole pattern and matches nothing — including the names
// CPython accepts. The fixture did not contain these rows, so both packages
// stayed green while they disagreed.
func TestCPythonNormalizesInvertedRanges(t *testing.T) {
	tests := []struct {
		pattern string
		name    string
		want    bool
	}{
		{"[c-a-e]", "e", true},
		{"[c-a-e]", "-", true},
		{"[c-a-e]", "a", false},
		{"[c-a-e]", "c", false},
		{"[a--c]", "c", true},
		{"[a--c]", "a", false},
		{"[a--c]", "-", false},
		{"[a--c]", "b", false},
		{"[z-a]", "z", false},
		{"[z-a]", "a", false},
		{"*[z-a]", "hello", false},
	}
	for _, tc := range tests {
		if got := Match(tc.pattern, tc.name); got != tc.want {
			t.Errorf("Match(%q, %q) = %v, want %v", tc.pattern, tc.name, got, tc.want)
		}
	}
}

// TestCaretInClassIsLiteralNotNegation pins CPython's semantics: only a
// leading '!' negates, and translate('[^a]') is the literal set {^, a}. Both
// planes once negated on '^', which made them disagree with the incumbent
// about which paths a [^…] rule named — in opposite directions.
func TestCaretInClassIsLiteralNotNegation(t *testing.T) {
	tests := []struct {
		pattern string
		name    string
		want    bool
	}{
		{"[^a]", "b", false}, // literal set {^, a} does not contain b
		{"[^a]", "^", true},  // ^ is a member
		{"[^a]", "a", true},
		{"[!a]", "b", true},  // ! still negates
		{"[!a]", "a", false}, //
		{"[^abc]", "^", true},
		{".env[^1]", ".env2", false},
	}
	for _, tc := range tests {
		if got := Match(tc.pattern, tc.name); got != tc.want {
			t.Errorf("Match(%q, %q) = %v, want %v", tc.pattern, tc.name, got, tc.want)
		}
	}
}

func TestMatchFoldFoldsCase(t *testing.T) {
	tests := []struct {
		pattern string
		name    string
		want    bool
	}{
		{".env", ".ENV", true},
		{".env", ".Env", true},
		{"*.pem", "A/B/FILE.PEM", true},
		{"**/.env", "a/b/.ENV", true},
		{"**/.env", ".ENV", false},
		{"[a-c]", "B", true},
		{"[A-C]", "b", true},
		{"[a-c]", "D", false},
		{"[!a]", "A", false},
		{"[!a]", "B", true},
		{"[^a]", "A", true},
		{"[^a]", "b", false},
		{"é", "É", true},
		{"café", "CAFÉ", true},
		// Simple case fold does not expand ß to "ss". Matching it would let a
		// two-character name satisfy a one-character pattern.
		{"ß", "SS", false},
		{"ß", "ß", true},
		// Dotted capital I folds to i. Over-blocking a name that differs only
		// by that letter is the cost MatchFold already accepts.
		{"i", "İ", true},
		{"İ", "i", true},
		{"[c-a-e]", "E", true},
		{"[a--c]", "C", true},
	}
	for _, tc := range tests {
		if got := MatchFold(tc.pattern, tc.name); got != tc.want {
			t.Errorf("MatchFold(%q, %q) = %v, want %v", tc.pattern, tc.name, got, tc.want)
		}
	}
}

func TestQuoteMetaMatchesOnlyTheLiteral(t *testing.T) {
	literals := []string{"a[bc].go", "a*b", "a?b", "plain.go", "a]b", "*", "[]", ""}
	for _, literal := range literals {
		quoted := QuoteMeta(literal)
		if !Match(quoted, literal) {
			t.Errorf("QuoteMeta(%q) = %q does not match the literal", literal, quoted)
		}
	}
	quoted := QuoteMeta("a[bc].go")
	for _, sibling := range []string{"ab.go", "ac.go", "abc.go", "a.go"} {
		if Match(quoted, sibling) {
			t.Errorf("QuoteMeta(%q) also matched %q", "a[bc].go", sibling)
		}
	}
}

func TestInvalidUTF8IsOneRunePerBadByte(t *testing.T) {
	if !Match("?", "\xff") {
		t.Fatal(`Match("?", "\xff") = false; one invalid byte is one rune`)
	}
	if Match("?", "\xff\xfe") {
		t.Fatal(`Match("?", "\xff\xfe") = true; two invalid bytes are two runes`)
	}
	if !Match("*.pem", "a\xff.pem") {
		t.Fatal(`a secret suffix after an invalid byte must still match`)
	}
	if Match("*.pem", "a\xffpem") {
		t.Fatal(`"*.pem" matched a name that does not end in .pem`)
	}
}

func TestOversizeFailsClosed(t *testing.T) {
	name := strings.Repeat("a", maxUnits+1)
	if Match("*", name) {
		t.Fatal("Match on an oversize name returned true; an allow-list would open")
	}
	if !MatchFold("*", name) {
		t.Fatal("MatchFold on an oversize name returned false; a deny-rule would miss it")
	}
	if !Match("*", strings.Repeat("a", maxUnits)) {
		t.Fatal("a star did not match a name sitting on the cap")
	}
	// Byte cap, before the rune conversion allocates. U+1F4A9 is four bytes,
	// so this is far past maxUnits*4 bytes and must not be walked.
	huge := strings.Repeat("💩", maxUnits+1)
	if Match("*", huge) {
		t.Fatal("Match walked a name past the byte cap")
	}
	if !MatchFold(".env", huge) {
		t.Fatal("MatchFold did not fail closed on a name past the byte cap")
	}
}

func TestManyStarsFinish(t *testing.T) {
	pattern := strings.Repeat("*a", 32) + "*b"
	name := strings.Repeat("a", 2000)
	start := time.Now()
	if Match(pattern, name) {
		t.Fatal("pattern should not match a name with no b")
	}
	if time.Since(start) > 2*time.Second {
		t.Fatalf("many-star match took %s; the backtrack is no longer bounded", time.Since(start))
	}
	if !Match(pattern, name+"b") {
		t.Fatal("the same pattern should match once a b is present")
	}
}

func TestMatchConcurrent(t *testing.T) {
	var wg sync.WaitGroup
	errCh := make(chan string, 1)
	for i := 0; i < 32; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for j := 0; j < 100; j++ {
				if !Match("*.py", "src/deep/foo.py") || Match("**/.env", ".env") || !MatchFold(".env", ".ENV") {
					select {
					case errCh <- "concurrent match disagreed with the single-threaded cases":
					default:
					}
					return
				}
			}
		}()
	}
	wg.Wait()
	select {
	case msg := <-errCh:
		t.Fatal(msg)
	default:
	}
}
