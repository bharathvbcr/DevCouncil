package lappi

import (
	"bytes"
	"strconv"
	"strings"
	"unicode/utf8"
)

// MaxContextBytes is the runtime's context cap, RenderCaps::DEFAULT
// .max_context_bytes = 131072 (Lappi-decision crates/qd-runtime/src/render.rs:149).
// A larger context is refused by the runtime (context_over_cap), so it is not
// sent.
const MaxContextBytes = 131_072

// Why a file was not asked about. The first four are the diff's own; the rest
// are the caller's bounds.
const (
	SkipBinary       = "binary"
	SkipDeleted      = "deleted"
	SkipOverCap      = "over_cap"
	SkipUnsplittable = "unsplittable"
	SkipAskingOff    = "asking_disabled"
	SkipBudgetSpent  = "budget_spent"
	SkipCancelled    = "cancelled"
)

// FileDiff is one file's section of a unified diff.
type FileDiff struct {
	// Path is the post-image path. It is used for the gap's File and for the
	// context's `file:` line, and never written into a caller record.
	Path string
	// Language is the extension's language in qd-lang's vocabulary, or
	// "unknown"; it is all of the path a caller record carries.
	Language string
	Hunks    int
	Added    int
	Deleted  int
	// Context is the trained context: `file: <path>`, a blank line, the
	// hunks. Nil when Skip is set.
	Context []byte
	// Skip is empty when the file can be asked about.
	Skip string
}

// LanguageOf maps a path's extension to the language names qd-lang uses
// (crates/qd-lang/src/lib.rs language_from_path): a .d.ts declaration has
// none, and an extension outside the map is "unknown". The extension itself is
// never returned, because an extension is part of a name.
func LanguageOf(path string) string {
	if strings.HasSuffix(path, ".d.ts") {
		return "unknown"
	}
	base := path
	if i := strings.LastIndexByte(base, '/'); i >= 0 {
		base = base[i+1:]
	}
	dot := strings.LastIndexByte(base, '.')
	if dot <= 0 {
		return "unknown"
	}
	switch base[dot+1:] {
	case "rs":
		return "rust"
	case "go":
		return "go"
	case "py", "pyi":
		return "python"
	case "ts", "tsx", "mts", "cts":
		return "typescript"
	case "swift":
		return "swift"
	default:
		return "unknown"
	}
}

// SplitDiff splits `git diff` output into one FileDiff per `diff --git`
// section, in order. A section is askable only when it carries a `+++ b/<path>`
// line and hunks whose bodies match their headers exactly (the check
// admission.rs runs on the other side); anything else is skipped with a reason
// rather than sent in a shape the model was never trained on.
func SplitDiff(diff []byte) []FileDiff {
	body := bytes.TrimSuffix(diff, []byte("\n"))
	if len(bytes.TrimSpace(body)) == 0 {
		return nil
	}
	lines := bytes.Split(body, []byte("\n"))
	var sections [][][]byte
	for _, line := range lines {
		if bytes.HasPrefix(line, []byte("diff --git ")) {
			sections = append(sections, [][]byte{line})
			continue
		}
		// Lines before the first section header belong to no file.
		if len(sections) > 0 {
			last := len(sections) - 1
			sections[last] = append(sections[last], line)
		}
	}
	out := make([]FileDiff, 0, len(sections))
	for _, section := range sections {
		out = append(out, splitSection(section))
	}
	return out
}

func splitSection(section [][]byte) FileDiff {
	var (
		newPath, oldPath string
		hasNew           bool
		binary, deleted  bool
		quoted           bool
		firstHunk        = -1
	)
	for i, line := range section[1:] {
		switch {
		case bytes.HasPrefix(line, []byte("@@ ")):
			firstHunk = i + 1
		case bytes.HasPrefix(line, []byte("+++ ")):
			hasNew = true
			p, q, isNull := headerPath(line[4:], "b/")
			newPath, quoted, deleted = p, quoted || q, deleted || isNull
		case bytes.HasPrefix(line, []byte("--- ")):
			p, q, _ := headerPath(line[4:], "a/")
			oldPath, quoted = p, quoted || q
		case bytes.HasPrefix(line, []byte("deleted file mode")):
			deleted = true
		case bytes.HasPrefix(line, []byte("Binary files ")), bytes.HasPrefix(line, []byte("GIT binary patch")):
			binary = true
		}
		if firstHunk >= 0 {
			break
		}
	}

	shown := newPath
	if shown == "" {
		shown = oldPath
	}
	if shown == "" {
		shown = string(section[0])
	}
	fd := FileDiff{Path: newPath, Language: LanguageOf(shown)}
	if newPath == "" {
		fd.Path = oldPath
	}
	switch {
	case binary:
		fd.Skip = SkipBinary
		return fd
	case deleted:
		fd.Skip = SkipDeleted
		return fd
	case quoted || !hasNew || newPath == "" || firstHunk < 0:
		fd.Skip = SkipUnsplittable
		return fd
	case !utf8.ValidString(newPath) || strings.TrimSpace(newPath) == "" || strings.ContainsAny(newPath, "\r\n"):
		fd.Skip = SkipUnsplittable
		return fd
	}

	hunks := section[firstHunk:]
	counts, ok := checkHunks(hunks)
	if !ok {
		fd.Skip = SkipUnsplittable
		return fd
	}
	fd.Hunks, fd.Added, fd.Deleted = counts.hunks, counts.added, counts.deleted

	var buf bytes.Buffer
	buf.WriteString("file: ")
	buf.WriteString(newPath)
	buf.WriteString("\n\n")
	// No trailing newline: the trained contexts drop it (admission.rs:44).
	buf.Write(bytes.Join(hunks, []byte("\n")))
	if buf.Len() > MaxContextBytes {
		fd.Skip = SkipOverCap
		return fd
	}
	fd.Context = buf.Bytes()
	return fd
}

// headerPath reads the path off a `---`/`+++` line. It reports a quoted path
// (git's C-style quoting, which this client does not undo) and /dev/null. A
// path without the expected prefix (diff.noprefix, diff.mnemonicPrefix) comes
// back empty, which makes the section unsplittable.
func headerPath(rest []byte, prefix string) (path string, quoted, devNull bool) {
	s := string(rest)
	// git appends a tab to a path that contains a space, so `patch` can find
	// the name's end.
	s = strings.TrimSuffix(s, "\t")
	if s == "/dev/null" {
		return "", false, true
	}
	if strings.HasPrefix(s, `"`) {
		return "", true, false
	}
	if !strings.HasPrefix(s, prefix) {
		return "", false, false
	}
	return strings.TrimPrefix(s, prefix), false, false
}

type hunkCounts struct{ hunks, added, deleted int }

// maxHunkCount bounds a header's declared counts; a larger number is not a
// diff this client produced from a working tree.
const maxHunkCount = 1 << 31

// checkHunks holds every hunk body to its header, the rules of admission.rs
// admit_defect_context: a header `@@ -a[,b] +c[,d] @@[ section]`, body lines
// starting with ' ', '-', '+' or '\', and exactly the declared old and new
// line counts.
func checkHunks(lines [][]byte) (hunkCounts, bool) {
	var c hunkCounts
	oldLeft, newLeft := 0, 0
	open := false
	for _, line := range lines {
		if bytes.HasPrefix(line, []byte("@@ ")) {
			if open && (oldLeft != 0 || newLeft != 0) {
				return c, false
			}
			o, n, ok := parseHunkHeader(line)
			if !ok {
				return c, false
			}
			oldLeft, newLeft, open = o, n, true
			c.hunks++
			continue
		}
		if !open || len(line) == 0 {
			return c, false
		}
		var o, n int
		switch line[0] {
		case ' ':
			o, n = 1, 1
		case '-':
			o, n = 1, 0
			c.deleted++
		case '+':
			o, n = 0, 1
			c.added++
		case '\\':
		default:
			return c, false
		}
		if o > oldLeft || n > newLeft {
			return c, false
		}
		oldLeft -= o
		newLeft -= n
	}
	if !open || oldLeft != 0 || newLeft != 0 {
		return c, false
	}
	return c, true
}

// parseHunkHeader reads `@@ -a[,b] +c[,d] @@[ section]` and returns (b, d).
func parseHunkHeader(line []byte) (int, int, bool) {
	rest, ok := bytes.CutPrefix(line, []byte("@@ -"))
	if !ok {
		return 0, 0, false
	}
	old, rest, ok := hunkRange(rest)
	if !ok {
		return 0, 0, false
	}
	rest, ok = bytes.CutPrefix(rest, []byte(" +"))
	if !ok {
		return 0, 0, false
	}
	nw, rest, ok := hunkRange(rest)
	if !ok {
		return 0, 0, false
	}
	rest, ok = bytes.CutPrefix(rest, []byte(" @@"))
	if !ok || (len(rest) > 0 && rest[0] != ' ') {
		return 0, 0, false
	}
	return old, nw, true
}

// hunkRange reads `digits[,digits]` and returns the count (absent = 1).
func hunkRange(s []byte) (int, []byte, bool) {
	number := func(s []byte) (int, []byte, bool) {
		end := 0
		for end < len(s) && s[end] >= '0' && s[end] <= '9' {
			end++
		}
		if end == 0 {
			return 0, s, false
		}
		v, err := strconv.ParseInt(string(s[:end]), 10, 64)
		if err != nil || v > maxHunkCount {
			return 0, s, false
		}
		return int(v), s[end:], true
	}
	if _, rest, ok := number(s); !ok {
		return 0, s, false
	} else if after, comma := bytes.CutPrefix(rest, []byte(",")); comma {
		return number(after)
	} else {
		return 1, rest, true
	}
}
