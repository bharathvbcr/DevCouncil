package lappi

import (
	"bytes"
	"strings"
	"testing"
)

// multiFileDiff is `git diff HEAD` over seven files: two askable, one new file
// whose name has a space, and four that must be skipped.
const multiFileDiff = "" +
	"diff --git a/src/add.go b/src/add.go\n" +
	"index 1111111..2222222 100644\n" +
	"--- a/src/add.go\n" +
	"+++ b/src/add.go\n" +
	"@@ -1,3 +1,3 @@ package add\n" +
	" package add\n" +
	"-func Add(a, b int) int { return a + b }\n" +
	"+func Add(a, b int) int { return a - b }\n" +
	" \n" +
	"diff --git a/pkg/m.py b/pkg/m.py\n" +
	"index 3333333..4444444 100644\n" +
	"--- a/pkg/m.py\n" +
	"+++ b/pkg/m.py\n" +
	"@@ -1,2 +1,2 @@\n" +
	"-x = 1\n" +
	"+x = 2\n" +
	" y = 3\n" +
	"@@ -10 +10,2 @@ def f():\n" +
	"     return x\n" +
	"+pass\n" +
	"\\ No newline at end of file\n" +
	"diff --git a/old.rs b/old.rs\n" +
	"deleted file mode 100644\n" +
	"index 5555555..0000000\n" +
	"--- a/old.rs\n" +
	"+++ /dev/null\n" +
	"@@ -1 +0,0 @@\n" +
	"-fn main() {}\n" +
	"diff --git a/img.png b/img.png\n" +
	"index 6666666..7777777 100644\n" +
	"Binary files a/img.png and b/img.png differ\n" +
	"diff --git a/a.ts b/b.ts\n" +
	"similarity index 100%\n" +
	"rename from a.ts\n" +
	"rename to b.ts\n" +
	"diff --git \"a/sp\\303\\251.go\" \"b/sp\\303\\251.go\"\n" +
	"index 8888888..9999999 100644\n" +
	"--- \"a/sp\\303\\251.go\"\n" +
	"+++ \"b/sp\\303\\251.go\"\n" +
	"@@ -1 +1 @@\n" +
	"-a\n" +
	"+b\n" +
	"diff --git a/my file.ts b/my file.ts\n" +
	"new file mode 100644\n" +
	"index 0000000..aaaaaaa\n" +
	"--- /dev/null\n" +
	"+++ b/my file.ts\t\n" +
	"@@ -0,0 +1 @@\n" +
	"+export const x = 1;\n"

const goldenGoContext = "file: src/add.go\n\n" +
	"@@ -1,3 +1,3 @@ package add\n" +
	" package add\n" +
	"-func Add(a, b int) int { return a + b }\n" +
	"+func Add(a, b int) int { return a - b }\n" +
	" "

// goldenGoRequest is the exact request line for goldenGoContext. context_b64
// was computed independently (Python base64.b64encode of the same bytes).
const goldenGoRequest = `{"schema_version":1,"task":"code.defect_class",` +
	`"context_b64":"ZmlsZTogc3JjL2FkZC5nbwoKQEAgLTEsMyArMSwzIEBAIHBhY2thZ2UgYWRkCiBwYWNrYWdlIGFkZAotZnVuYyBBZGQoYSwgYiBpbnQpIGludCB7IHJldHVybiBhICsgYiB9CitmdW5jIEFkZChhLCBiIGludCkgaW50IHsgcmV0dXJuIGEgLSBiIH0KIA==",` +
	`"context_len":142,` +
	`"question":"What kind of change is this diff, and which lines does it touch?",` +
	`"slots":[{"name":"defect_class","type":"choice","options":["stub","logic","cosmetic","clean"]},{"name":"defect_span","type":"span"}],` +
	`"route":"generic","example_id":"devcouncil:code.defect_class:c0ffee00c0ffee00c0ffee00c0ffee00","metadata":{}}`

func TestAMultiFileDiffSplitsIntoPerFileContextsWithNoPreamble(t *testing.T) {
	files := SplitDiff([]byte(multiFileDiff))
	type want struct {
		path, lang, skip      string
		hunks, added, deleted int
	}
	wants := []want{
		{"src/add.go", "go", "", 1, 1, 1},
		{"pkg/m.py", "python", "", 2, 2, 1},
		{"old.rs", "rust", SkipDeleted, 0, 0, 0},
		{"", "unknown", SkipBinary, 0, 0, 0},
		{"", "typescript", SkipUnsplittable, 0, 0, 0},
		// A quoted path is not unquoted, so its language is not guessed.
		{"", "unknown", SkipUnsplittable, 0, 0, 0},
		{"my file.ts", "typescript", "", 1, 1, 0},
	}
	if len(files) != len(wants) {
		t.Fatalf("got %d files, want %d: %+v", len(files), len(wants), files)
	}
	for i, w := range wants {
		f := files[i]
		if f.Path != w.path || f.Language != w.lang || f.Skip != w.skip ||
			f.Hunks != w.hunks || f.Added != w.added || f.Deleted != w.deleted {
			t.Errorf("file %d: got %+v, want %+v", i, f, w)
		}
		if (f.Skip == "") != (f.Context != nil) {
			t.Errorf("file %d: a context exactly when askable, got skip=%q context=%v", i, f.Skip, f.Context != nil)
		}
		for _, line := range bytes.Split(f.Context, []byte("\n")) {
			for _, preamble := range []string{"diff --git", "--- ", "+++ ", "index ", "new file mode"} {
				if bytes.HasPrefix(line, []byte(preamble)) {
					t.Errorf("file %d: context carries preamble line %q", i, line)
				}
			}
		}
	}
	if got := string(files[0].Context); got != goldenGoContext {
		t.Fatalf("go context:\n%q\nwant\n%q", got, goldenGoContext)
	}
	wantPy := "file: pkg/m.py\n\n@@ -1,2 +1,2 @@\n-x = 1\n+x = 2\n y = 3\n@@ -10 +10,2 @@ def f():\n     return x\n+pass\n\\ No newline at end of file"
	if got := string(files[1].Context); got != wantPy {
		t.Fatalf("python context:\n%q\nwant\n%q", got, wantPy)
	}
	if got := string(files[6].Context); !strings.HasPrefix(got, "file: my file.ts\n\n@@ -0,0 +1 @@") {
		t.Fatalf("the tab git appends to a spaced path must not reach the context: %q", got)
	}
}

func TestTheDefectClassRequestIsTheTrainedRequestByteForByte(t *testing.T) {
	line, err := NewDefectClassRequest([]byte(goldenGoContext), "c0ffee00c0ffee00c0ffee00c0ffee00").Encode()
	if err != nil {
		t.Fatal(err)
	}
	if string(line) != goldenGoRequest {
		t.Fatalf("request line:\n%s\nwant\n%s", line, goldenGoRequest)
	}
}

func TestAHunkThatDoesNotMatchItsHeaderIsNotSent(t *testing.T) {
	head := "diff --git a/a.go b/a.go\n--- a/a.go\n+++ b/a.go\n"
	for name, body := range map[string]string{
		"short_body":        "@@ -1,2 +1,2 @@\n-a\n+b\n",
		"long_body":         "@@ -1 +1 @@\n-a\n+b\n+c\n",
		"empty_line":        "@@ -1,2 +1,2 @@\n a\n\n-b\n+c\n",
		"garbage_line":      "@@ -1 +1 @@\n-a\n!b\n",
		"crlf_header":       "@@ -1 +1 @@\r\n-a\n+b\n",
		"header_suffix":     "@@ -1 +1 @@x\n-a\n+b\n",
		"count_overflow":    "@@ -99999999999999999999,1 +1 @@\n-a\n+b\n",
		"missing_counts":    "@@ -,1 +1 @@\n-a\n+b\n",
		"second_hunk_short": "@@ -1 +1 @@\n-a\n+b\n@@ -5,2 +5,2 @@\n x\n",
		"stray_after_hunk":  "@@ -1 +1 @@\n-a\n+b\nindex 1..2\n",
	} {
		files := SplitDiff([]byte(head + body))
		if len(files) != 1 || files[0].Skip != SkipUnsplittable || files[0].Context != nil {
			t.Errorf("%s: got %+v", name, files)
		}
	}
	noPrefix := "diff --git a/a.go b/a.go\n--- a.go\n+++ a.go\n@@ -1 +1 @@\n-a\n+b\n"
	if f := SplitDiff([]byte(noPrefix)); len(f) != 1 || f[0].Skip != SkipUnsplittable {
		t.Errorf("a diff.noprefix path is not split: %+v", f)
	}
}

func TestAContextOverTheRuntimeCapIsNotSent(t *testing.T) {
	var b strings.Builder
	b.WriteString("diff --git a/a.go b/a.go\n--- a/a.go\n+++ b/a.go\n@@ -0,0 +1,2000 @@\n")
	for i := 0; i < 2000; i++ {
		b.WriteString("+" + strings.Repeat("x", 80) + "\n")
	}
	files := SplitDiff([]byte(b.String()))
	if len(files) != 1 || files[0].Skip != SkipOverCap || files[0].Context != nil {
		t.Fatalf("got %+v", files[0].Skip)
	}
}

func TestAnEmptyOrPreambleOnlyDiffHasNoFiles(t *testing.T) {
	for _, d := range []string{"", "\n", "   \n", "warning: something\n"} {
		if files := SplitDiff([]byte(d)); len(files) != 0 {
			t.Errorf("%q: got %+v", d, files)
		}
	}
}

func TestLanguageNamesNeverEchoTheExtension(t *testing.T) {
	for path, want := range map[string]string{
		"a/b.rs": "rust", "b.go": "go", "c.py": "python", "c.pyi": "python",
		"d.ts": "typescript", "e.tsx": "typescript", "f.mts": "typescript", "g.cts": "typescript",
		"h.swift": "swift", "types.d.ts": "unknown", "Makefile": "unknown", ".gitignore": "unknown",
		"dir.go/readme": "unknown", "project.acmecorpsecret": "unknown",
	} {
		if got := LanguageOf(path); got != want {
			t.Errorf("LanguageOf(%q) = %q, want %q", path, got, want)
		}
	}
}
