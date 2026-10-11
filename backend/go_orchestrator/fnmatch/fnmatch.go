// Package fnmatch implements Python's fnmatch semantics.
//
// This exists because Go's path.Match and Python's fnmatch disagree on the one
// question the write gate depends on: whether "*" crosses a path separator.
//
//	Python:  fnmatch("src/foo.py", "*.py")  == True
//	Go:      path.Match("*.py", "src/foo.py") == false
//
// Every DevCouncil path rule — the secret patterns, the restricted paths, the
// planned-file globs, forbidden_changes — is a Python fnmatch pattern. Porting
// them onto path.Match would narrow the secret and restricted rules (a gate
// that stops denying) and narrow planned-file matching (a gate that starts
// denying legitimate writes). Both failures are silent.
//
// The matcher is the same backtracking algorithm as rust/dc-glob. The previous
// Go copy translated patterns into RE2, and RE2 rejects character classes
// CPython normalises (an out-of-order range such as [c-a-e] still matches "e"
// and "-"). That translation matched nothing for the whole pattern, so the two
// planes disagreed on exactly the paths a class named. One algorithm, pinned
// by testdata/fnmatch-parity.tsv, which scripts/gen-fnmatch-parity.py
// regenerates from CPython's fnmatchcase.
//
// Both languages still run the algorithm in-process. The write gate cannot
// cross into the Rust crate: a transport failure has no honest bool, and
// either choice (match, or not) is a wrong allow or a wrong deny. dc-verify
// cannot call this package. The shared fixture is what keeps the two copies
// from drifting.
//
// Match is case-sensitive, which is what Python's fnmatchcase does, and what
// fnmatch does on POSIX where os.path.normcase is the identity. DevCouncil's
// own patterns are checked this way on macOS and Linux today, and the parity
// fixture pins it.
//
// MatchFold is the deliberate exception, and it is a divergence from the
// incumbent rather than a port of it. Case-sensitive matching is a statement
// about strings; a write gate needs a statement about files. On APFS and NTFS —
// the default filesystems on two of the three platforms this runs on — ".ENV"
// and ".env" are the same file, so a case-sensitive secret-path check reads the
// pattern list and still lets the write through. Hard rules are matched with
// MatchFold for that reason. The cost is over-blocking a file that differs from
// a credential only by case, which is not a file anyone needs to write.
package fnmatch

import (
	"strings"
	"unicode"
	"unicode/utf8"
)

// maxUnits is the longest pattern or name, in Unicode scalar values, that is
// matched exactly. Past it the result is the fail-closed bool for that entry
// point: Match returns false (an allow-list miss denies), MatchFold returns
// true (a deny-rule miss would allow). A real repository path is far below
// this. The bound also caps the backtrack, which is O(pattern × name).
const maxUnits = 16384

// Match reports whether name matches the shell-style pattern, using Python's
// fnmatchcase rules. A pattern that cannot be applied never matches; it cannot
// panic and it cannot accidentally match everything.
func Match(pattern, name string) bool {
	return decide(pattern, name, false)
}

// MatchAny reports whether name matches any of the patterns.
func MatchAny(patterns []string, name string) bool {
	for _, p := range patterns {
		if Match(p, name) {
			return true
		}
	}
	return false
}

// MatchFold reports whether name matches pattern ignoring case, using Unicode
// simple case folding (one rune to one rune). Use it wherever a mismatch would
// let a write reach a file the pattern was written to protect; see the package
// comment.
func MatchFold(pattern, name string) bool {
	if oversized(pattern) || oversized(name) {
		return true
	}
	return decide(fold(pattern), fold(name), true)
}

// MatchAnyFold reports whether name matches any pattern, ignoring case.
func MatchAnyFold(patterns []string, name string) bool {
	for _, p := range patterns {
		if MatchFold(p, name) {
			return true
		}
	}
	return false
}

// QuoteMeta escapes a literal string so it matches only itself when used as a
// pattern.
//
// This is needed wherever a concrete path becomes a pattern — the override
// seam builds a grant's scope from the path that was blocked. Without it, a
// real file named "a[bc].go" yields a grant that also covers "ab.go" and
// "ac.go", so clearing one block silently clears three. Bracket escaping uses
// a single-character class rather than a backslash because that is what
// Python's fnmatch understands; a backslash is a literal there, not an escape.
func QuoteMeta(literal string) string {
	var b strings.Builder
	b.Grow(len(literal))
	for _, r := range literal {
		switch r {
		case '*', '?', '[':
			b.WriteByte('[')
			b.WriteRune(r)
			b.WriteByte(']')
		default:
			b.WriteRune(r)
		}
	}
	return b.String()
}

func decide(pattern, name string, oversizeIsMatch bool) bool {
	if oversized(pattern) || oversized(name) {
		return oversizeIsMatch
	}
	pat := []rune(pattern)
	text := []rune(name)
	matched, completed := matchFrom(pat, text)
	if !completed {
		return oversizeIsMatch
	}
	return matched
}

// Oversized reports whether s is past the length matched exactly (maxUnits
// Unicode scalar values). Past it, Match answers false and MatchFold true
// without walking. Exported so a matcher answering for this package — the
// in-process engine behind gussetfn — applies the same rule.
func Oversized(s string) bool { return oversized(s) }

// Fold is the case folding MatchFold applies to both arguments, exported for
// the same reason as Oversized.
func Fold(s string) string { return fold(s) }

func oversized(s string) bool {
	if len(s) > maxUnits*utf8.UTFMax {
		return true
	}
	return utf8.RuneCountInString(s) > maxUnits
}

func fold(s string) string {
	rs := []rune(s)
	changed := false
	for i, r := range rs {
		if folded := foldRune(r); folded != r {
			rs[i] = folded
			changed = true
		}
	}
	if !changed {
		return s
	}
	return string(rs)
}

// foldRune is simple case folding: the one scalar a case-insensitive
// filesystem would compare r as. unicode.ToLower is not that. APFS and NTFS
// fold by the Unicode orbit, and two scalars outside ASCII share one with an
// ASCII letter — U+017F LATIN SMALL LETTER LONG S folds to "s" and U+212A KELVIN
// SIGN to "k" — so `id_rſa` and `ſecrets/x` name `id_rsa` and `secrets/x` on
// disk while ToLower left the first as it was and every secret pattern missed
// it. ToLower already maps the Kelvin sign; the long s is the one it does not.
//
// Every other orbit is folded by ToLower, which is enough for the patterns the
// gates hold (all ASCII) and is what dc-glob does; TestFoldCoversEveryASCIIOrbit
// walks the whole Unicode range for the scalars whose orbit holds an ASCII
// letter, so a Unicode revision that adds one fails the test rather than the gate.
func foldRune(r rune) rune {
	if r < utf8.RuneSelf {
		return unicode.ToLower(r)
	}
	lower := unicode.ToLower(r)
	if lower < utf8.RuneSelf {
		return lower
	}
	for f := unicode.SimpleFold(lower); f != lower; f = unicode.SimpleFold(f) {
		if f < utf8.RuneSelf && f >= 'A' && f <= 'Z' {
			return unicode.ToLower(f)
		}
		if f < utf8.RuneSelf && f >= 'a' && f <= 'z' {
			return f
		}
	}
	return lower
}

// matchFrom reports whether text matches pat, and whether the walk finished
// inside its step budget. A false second result means the budget tripped; the
// caller then applies its fail-closed bool instead of trusting a partial walk.
func matchFrom(pat, text []rune) (matched, completed bool) {
	pi, ti := 0, 0
	starPi, starTi := -1, -1
	// Each step either consumes one pattern unit or gives the latest star one
	// more character. (len+1)² covers that; exceeding it is a loop bug.
	steps := 0
	// Four times the one-step-per-unit bound. The walk is O(pattern × name);
	// the multiplier is slack for the star-collapse iterations inside a step,
	// not a second algorithm.
	limit := 4*((len(pat)+1)*(len(text)+1)) + 8

	for {
		steps++
		if steps > limit {
			return false, false
		}
		if pi < len(pat) {
			switch pat[pi] {
			case '*':
				// Consecutive stars collapse, so "**/x" is "*" then "/x" —
				// "**/.env" needs a separator before ".env" and does not match
				// a bare ".env".
				for pi < len(pat) && pat[pi] == '*' {
					pi++
				}
				starPi, starTi = pi, ti
				continue
			case '?':
				if ti < len(text) {
					pi++
					ti++
					continue
				}
			case '[':
				if class, next, ok := parseClass(pat, pi); ok {
					if ti < len(text) && class.contains(text[ti]) {
						pi = next + 1
						ti++
						continue
					}
				} else if ti < len(text) && text[ti] == '[' {
					// Unterminated '[' is a literal, never a wildcard.
					pi++
					ti++
					continue
				}
			default:
				if ti < len(text) && text[ti] == pat[pi] {
					pi++
					ti++
					continue
				}
			}
		} else if ti == len(text) {
			return true, true
		}

		if starPi >= 0 && starTi < len(text) {
			starTi++
			pi = starPi
			ti = starTi
			continue
		}
		return false, true
	}
}

type charClass struct {
	negated bool
	singles []rune
	ranges  [][2]rune
}

func (c charClass) contains(r rune) bool {
	hit := false
	for _, s := range c.singles {
		if s == r {
			hit = true
			break
		}
	}
	if !hit {
		for _, rg := range c.ranges {
			if r >= rg[0] && r <= rg[1] {
				hit = true
				break
			}
		}
	}
	return hit != c.negated
}

// parseClass parses the class starting at pat[open]. The returned index is the
// closing bracket. An unterminated or empty class is not a class: the caller
// treats '[' as a literal.
//
// Only a leading '!' negates. A leading '^' is an ordinary member of the set,
// which is what CPython's fnmatch does. A range is recognised the same way the
// Rust matcher does: three units "x-y" with room after the start. An
// out-of-order range matches nothing and leaves the surrounding members, which
// is how "[c-a-e]" still matches "e" and "-".
func parseClass(pat []rune, open int) (charClass, int, bool) {
	i := open + 1
	negated := i < len(pat) && pat[i] == '!'
	if negated {
		i++
	}
	bodyStart := i
	// A ']' immediately after the opening (or after the negation) is a literal.
	if i < len(pat) && pat[i] == ']' {
		i++
	}
	for i < len(pat) && pat[i] != ']' {
		i++
	}
	if i >= len(pat) {
		return charClass{}, 0, false
	}
	body := pat[bodyStart:i]
	if len(body) == 0 {
		return charClass{}, 0, false
	}

	if indexRune(body, '-', 0) >= 0 {
		return dashedClass(negated, body), i, true
	}
	singles := append([]rune(nil), body...)
	var ranges [][2]rune
	return charClass{negated: negated, singles: singles, ranges: ranges}, i, true
}

// dashedClass reads a set that holds a "-" the way CPython's fnmatch.translate
// does, which is not the way a regular-expression parser would read the text:
// translate cuts the set into chunks at each "-" that could be a range
// operator, drops every range whose start is after its end by merging the two
// chunks around it — taking BOTH endpoints with it — and only then hands the
// joined chunks to the regex engine. `[]--!]` therefore loses the range `]`-`-`
// and with it the `-` and the `!` that followed, leaving a negation marker with
// no members, which matches any one character. Reading the set directly keeps
// the `!` and matches only it. The result is read back the way sre reads a set:
// a hyphen between two characters is a range operator unless the character
// before it ended a range.
func dashedClass(negated bool, body []rune) charClass {
	stuff := body
	if negated {
		stuff = append([]rune{'!'}, body...)
	}
	k := 1
	if negated {
		k = 2
	}
	var chunks [][]rune
	start := 0
	for {
		idx := indexRune(stuff, '-', k)
		if idx < 0 {
			break
		}
		chunks = append(chunks, stuff[start:idx])
		start = idx + 1
		k = idx + 3
	}
	if chunk := stuff[start:]; len(chunk) > 0 {
		chunks = append(chunks, chunk)
	} else {
		last := len(chunks) - 1
		chunks[last] = append(append([]rune(nil), chunks[last]...), '-')
	}
	for k := len(chunks) - 1; k > 0; k-- {
		// CPython raises IndexError when a merge empties a chunk that an
		// earlier pair then reads; such a pair is left as it is here.
		if len(chunks[k-1]) == 0 || len(chunks[k]) == 0 {
			continue
		}
		if chunks[k-1][len(chunks[k-1])-1] > chunks[k][0] {
			merged := append(append([]rune(nil), chunks[k-1][:len(chunks[k-1])-1]...), chunks[k][1:]...)
			chunks[k-1] = merged
			chunks = append(chunks[:k], chunks[k+1:]...)
		}
	}
	type token struct {
		r        rune
		operator bool
	}
	var tokens []token
	for n, chunk := range chunks {
		if n > 0 {
			tokens = append(tokens, token{'-', true})
		}
		for _, r := range chunk {
			tokens = append(tokens, token{r, false})
		}
	}
	class := charClass{}
	if len(tokens) > 0 && tokens[0].r == '!' && !tokens[0].operator {
		class.negated = true
		tokens = tokens[1:]
	}
	for t := 0; t < len(tokens); {
		if t+2 < len(tokens) && tokens[t+1].operator {
			class.ranges = append(class.ranges, [2]rune{tokens[t].r, tokens[t+2].r})
			t += 3
			continue
		}
		class.singles = append(class.singles, tokens[t].r)
		t++
	}
	return class
}

func indexRune(rs []rune, r rune, from int) int {
	for i := from; i < len(rs); i++ {
		if rs[i] == r {
			return i
		}
	}
	return -1
}
