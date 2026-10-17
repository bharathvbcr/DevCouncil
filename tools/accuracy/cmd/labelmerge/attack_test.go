package main

import (
	"bytes"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// Adversarial probes for labelmerge. A test that FAILS here is a break.

const edgeRow = `{"language":"go","caller":"a::F","callee":"a::G","correct":%s}`

func edge(correct string) string { return strings.Replace(edgeRow, "%s", correct, 1) }

// A duplicate key inside one object is last-wins in encoding/json: the file
// says two things about one site and the merge picks one without telling.
func TestAttackDuplicateKeyInOneObjectIsRefused(t *testing.T) {
	dir := write(t, map[string]string{
		"labels-go.json": `{"edges":[{"language":"go","caller":"a::F","callee":"a::G","correct":true,"correct":false}]}`,
	})
	out, _, err := Merge(dir, Options{})
	if err == nil {
		t.Fatalf("duplicate key `correct` accepted silently; merged:\n%s", out)
	}
}

func TestAttackDuplicateTopLevelKeyIsRefused(t *testing.T) {
	dir := write(t, map[string]string{
		"labels-go.json": `{"edges":[` + edge("true") + `],"edges":[` + edge("false") + `]}`,
	})
	out, _, err := Merge(dir, Options{})
	if err == nil {
		t.Fatalf("duplicate top-level key `edges` accepted silently (second array wins); merged:\n%s", out)
	}
}

// Conflict detection only compares ACROSS files. Two rows for one site inside
// one file that disagree are both emitted.
func TestAttackSameFileConflictingRowsAreRefused(t *testing.T) {
	dir := write(t, map[string]string{
		"labels-go.json": `{"edges":[` + edge("true") + `,` + edge("false") + `]}`,
	})
	out, _, err := Merge(dir, Options{})
	if err == nil {
		t.Fatalf("one file labels a site both correct and incorrect; both rows emitted:\n%s", out)
	}
}

func TestAttackSameFileConflictingOracleRowsAreRefused(t *testing.T) {
	dir := write(t, map[string]string{
		"labels-go.json": `{"oracle":[{"language":"go","covered_callers":["a"],"uses":[]},{"language":"go","covered_callers":["a","b"],"uses":[]}]}`,
	})
	out, _, err := Merge(dir, Options{})
	if err == nil {
		t.Fatalf("two different oracles for one language in one file both emitted:\n%s", out)
	}
}

// An identical duplicate inside one file is double counted by the runner's
// precision tally; across files the merge keeps it once, within a file it
// keeps it twice.
func TestAttackSameFileIdenticalDuplicateIsKeptOnce(t *testing.T) {
	dir := write(t, map[string]string{
		"labels-go.json": `{"edges":[` + edge("true") + `,` + edge("true") + `]}`,
	})
	out, counts, err := Merge(dir, Options{})
	if err != nil {
		t.Fatal(err)
	}
	if counts["go"] != 1 {
		t.Fatalf("identical duplicate row kept %d times (precision would double count it):\n%s", counts["go"], out)
	}
}

func TestAttackDegenerateFilesAreRefused(t *testing.T) {
	cases := map[string]string{
		"zero-byte":         "",
		"bom":               "\xef\xbb\xbf" + `{"edges":[` + edge("true") + `]}`,
		"json-null":         "null",
		"empty-object":      "{}",
		"empty-arrays":      `{"edges":[],"recall":[],"oracle":[],"dead":[]}`,
		"null-arrays":       `{"edges":null,"recall":null}`,
		"whitespace-only":   "  \n",
		"nul-byte-trailing": `{"edges":[` + edge("true") + `]}` + "\x00",
	}
	for name, content := range cases {
		t.Run(name, func(t *testing.T) {
			dir := write(t, map[string]string{"labels-go.json": content})
			out, counts, err := Merge(dir, Options{})
			if err == nil {
				t.Fatalf("accepted; counts=%v out=%s", counts, out)
			}
		})
	}
}

// "an empty store is refused" must mean a store with no labels, not only a
// store with no files.
func TestAttackStoreOfOnlyEmptyLabelFilesIsRefused(t *testing.T) {
	dir := write(t, map[string]string{
		"labels-go.json":     `{}`,
		"labels-python.json": `{"edges":[]}`,
	})
	var stdout, stderr bytes.Buffer
	code := run([]string{"-dir", dir}, &stdout, &stderr)
	if code == 0 {
		t.Fatalf("exit 0 with zero labels merged; stdout=%q stderr=%q", stdout.String(), stderr.String())
	}
}

// Misnamed files: only names that start `labels-` and end `.json`, exactly
// lower-case, are examined. Every other spelling is skipped without a word.
func TestAttackMisnamedLabelFilesAreNotSkippedSilently(t *testing.T) {
	good := `{"edges":[` + edge("true") + `]}`
	for _, bad := range []string{"Labels-go.json", "LABELS-go.json", "labels-go.JSON", "label-go.json", "labels_go.json", "labels-go.json.json.bak", "labels-go-r2.jsn"} {
		t.Run(bad, func(t *testing.T) {
			dir := write(t, map[string]string{
				"labels-go.json": good,
				bad:              `{"edges":[` + edge("false") + `]}`,
			})
			var stderr bytes.Buffer
			code := run([]string{"-dir", dir}, &bytes.Buffer{}, &stderr)
			if code == 0 && !strings.Contains(stderr.String(), bad) {
				t.Fatalf("%s was neither merged nor named; exit 0, stderr=%q", bad, stderr.String())
			}
		})
	}
}

func TestAttackRoundOrderingIsNumeric(t *testing.T) {
	dir := write(t, map[string]string{
		"labels-go-r10.json": `{}`, "labels-go-r2.json": `{}`, "labels-go-r1.json": `{}`, "labels-go-r02.json": `{}`,
	})
	files, err := inputFiles(dir)
	if err != nil {
		t.Fatal(err)
	}
	var got []string
	for _, f := range files {
		got = append(got, f.name)
	}
	want := "labels-go-r1.json labels-go-r02.json labels-go-r2.json labels-go-r10.json"
	if strings.Join(got, " ") != want {
		t.Fatalf("order = %v", got)
	}
}

func TestAttackHugeRoundNumberIsRefusedNotWrapped(t *testing.T) {
	dir := write(t, map[string]string{"labels-go-r99999999999999999999999.json": `{}`, "labels-go.json": `{}`})
	if _, err := inputFiles(dir); err == nil {
		t.Fatal("a round that overflows int was accepted")
	}
}

func TestAttackOutputThroughHardlinkAndSymlinkOfInputIsRefused(t *testing.T) {
	good := `{"edges":[` + edge("true") + `]}`
	dir := write(t, map[string]string{"labels-go.json": good})
	in := filepath.Join(dir, "labels-go.json")
	other := t.TempDir()
	hard := filepath.Join(other, "hard.json")
	if err := os.Link(in, hard); err != nil {
		t.Skip(err)
	}
	sym := filepath.Join(other, "sym.json")
	if err := os.Symlink(in, sym); err != nil {
		t.Fatal(err)
	}
	dangling := filepath.Join(other, "dangling.json")
	if err := os.Symlink(filepath.Join(dir, "labels-go-r2.json"), dangling); err != nil {
		t.Fatal(err)
	}
	spelled := filepath.Join(dir, "..", filepath.Base(dir), "labels-go.json")
	for name, out := range map[string]string{"hardlink": hard, "symlink": sym, "dotdot": spelled} {
		t.Run(name, func(t *testing.T) {
			var stderr bytes.Buffer
			if code := run([]string{"-dir", dir, "-o", out}, &bytes.Buffer{}, &stderr); code == 0 {
				t.Fatalf("exit 0; input was overwritten")
			}
			got, _ := os.ReadFile(in)
			if string(got) != good {
				t.Fatalf("input clobbered: %s", got)
			}
		})
	}
	// A dangling symlink whose target is a FUTURE input name (labels-go-r2.json):
	// os.Stat says not-exist, so the guard passes and WriteFile creates the
	// target inside the store through the link.
	t.Run("dangling-symlink-into-store", func(t *testing.T) {
		var stderr bytes.Buffer
		code := run([]string{"-dir", dir, "-o", dangling}, &bytes.Buffer{}, &stderr)
		_, statErr := os.Stat(filepath.Join(dir, "labels-go-r2.json"))
		if code == 0 && statErr == nil {
			t.Fatalf("merged output was written into the store as labels-go-r2.json via a dangling symlink")
		}
	})
}

// -o inside the store, under a name the store's own pattern matches, poisons
// the next merge (the output is read back as an input).
func TestAttackOutputInsideStoreDoesNotPoisonNextMerge(t *testing.T) {
	dir := write(t, map[string]string{"labels-go.json": `{"edges":[` + edge("true") + `]}`})
	out := filepath.Join(dir, "labels-merged.json")
	if code := run([]string{"-dir", dir, "-o", out}, &bytes.Buffer{}, &bytes.Buffer{}); code != 0 {
		t.Fatal("first merge failed")
	}
	var stderr bytes.Buffer
	if code := run([]string{"-dir", dir}, &bytes.Buffer{}, &stderr); code != 0 {
		t.Fatalf("the store is now unmergeable after labelmerge wrote into it: %s", stderr.String())
	}
}

// Atomic output: a temp file + rename replaces the inode. An in-place
// truncating WriteFile keeps it, so a failure (or a crash) mid-write leaves
// a truncated labels.json the runner will read.
func TestAttackOutputIsWrittenAtomically(t *testing.T) {
	dir := write(t, map[string]string{"labels-go.json": `{"edges":[` + edge("true") + `]}`})
	out := filepath.Join(t.TempDir(), "labels.json")
	if err := os.WriteFile(out, []byte("previous good labels\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	before, _ := os.Stat(out)
	keep := out + ".keep"
	if err := os.Link(out, keep); err != nil {
		t.Skip(err)
	}
	if code := run([]string{"-dir", dir, "-o", out}, &bytes.Buffer{}, &bytes.Buffer{}); code != 0 {
		t.Fatal("merge failed")
	}
	after, _ := os.Stat(out)
	if os.SameFile(before, after) {
		got, _ := os.ReadFile(keep)
		t.Fatalf("output rewritten in place (same inode; no temp+rename); a hardlink reader saw %q", got)
	}
}

// Not a Unicode-normalising merge: the same caller in NFC and NFD is two
// sites. Characterises behaviour; downstream the runner fails loudly on the
// spelling the graph does not have.
func TestAttackNFCvsNFDAreDistinctSites(t *testing.T) {
	nfc := "café"
	nfd := "café"
	dir := write(t, map[string]string{
		"labels-go.json":    `{"edges":[{"language":"go","caller":"` + nfc + `::F","callee":"a::G","correct":true}]}`,
		"labels-go-r2.json": `{"edges":[{"language":"go","caller":"` + nfd + `::F","callee":"a::G","correct":false}]}`,
	})
	_, counts, err := Merge(dir, Options{})
	if err == nil && counts["go"] == 2 {
		t.Logf("NFC and NFD callers kept as two contradictory sites (no error)")
	}
}

func TestAttackEscapedAndWhitespaceVariantsAreTheSameRow(t *testing.T) {
	dir := write(t, map[string]string{
		"labels-go.json":    `{"edges":[{"language":"go","caller":"a::F","callee":"a::G>","correct":true}]}`,
		"labels-go-r2.json": "{ \"edges\" : [ { \"callee\":\"a::G\\u003e\", \"language\":\"go\", \"correct\":true,\n\"caller\":\"a::F\" } ] }",
	})
	_, counts, err := Merge(dir, Options{})
	if err != nil || counts["go"] != 1 {
		t.Fatalf("err=%v counts=%v", err, counts)
	}
}

func TestAttackSymlinkLoopFileFailsLoud(t *testing.T) {
	dir := t.TempDir()
	if err := os.Symlink(filepath.Join(dir, "labels-go.json"), filepath.Join(dir, "labels-go.json")); err != nil {
		t.Fatal(err)
	}
	if _, _, err := Merge(dir, Options{}); err == nil {
		t.Fatal("a self-referential symlink merged")
	}
}

func TestAttackPrivateHashAndCategoryHoldAgainstOddShapes(t *testing.T) {
	deadRow := func(extra string) string {
		return `{"dead":[{"language":"go","id":"a::D","truly_dead":true` + extra + `}]}`
	}
	bad := map[string]string{
		"hash-null":      `,"symbol_hash":null,"category":"direct_reference"`,
		"hash-upper":     `,"symbol_hash":"h:0123456789AB","category":"direct_reference"`,
		"hash-13":        `,"symbol_hash":"h:0123456789abc","category":"direct_reference"`,
		"hash-newline":   `,"symbol_hash":"h:0123456789ab\n","category":"direct_reference"`,
		"cat-missing":    `,"symbol_hash":"h:0123456789ab"`,
		"cat-case":       `,"symbol_hash":"h:0123456789ab","category":"Direct_Reference"`,
		"cat-number":     `,"symbol_hash":"h:0123456789ab","category":1`,
		"hash-number":    `,"symbol_hash":12`,
		"nothing-at-all": ``,
	}
	for name, extra := range bad {
		t.Run(name, func(t *testing.T) {
			dir := write(t, map[string]string{"labels-go.json": deadRow(extra)})
			if _, _, err := Merge(dir, Options{Private: true}); err == nil {
				t.Fatal("accepted under -private")
			}
		})
	}
}

// No size bound: os.ReadFile plus map decoding of an arbitrarily large file.
func TestAttackLabelFileSizeIsBounded(t *testing.T) {
	if testing.Short() {
		t.Skip("allocates ~100MB")
	}
	pad := strings.Repeat("x", 48<<20)
	dir := write(t, map[string]string{
		"labels-go.json": `{"edges":[{"language":"go","caller":"a::F","callee":"a::G","correct":true,"pad":"` + pad + `"}]}`,
	})
	if _, _, err := Merge(dir, Options{}); err == nil {
		t.Fatal("a 48MB label file was read and merged with no size bound")
	}
}

func FuzzParseFile(f *testing.F) {
	for _, s := range []string{
		``, `{}`, `null`, `[]`, `{"edges":[` + edge("true") + `]}`,
		`{"dead":[{"language":"go","id":"a::D","truly_dead":true,"symbol_hash":"h:0123456789ab","category":"direct_reference"}]}`,
		`{"oracle":[{"language":"go","covered_callers":[],"uses":[]}]}`,
		`{"recall":[{"language":"go","construct":"c","caller":"a","name":"n","truth":"t"}]}`,
		"\xef\xbb\xbf{}", `{"edges":null}`, `{"edges":[null]}`, `{"edges":[{"language":"go"}]}`,
	} {
		f.Add(s, true)
	}
	f.Fuzz(func(t *testing.T, raw string, private bool) {
		rows, err := parseFile("labels-go.json", "go", []byte(raw), Options{Private: private})
		if err != nil {
			return
		}
		for s, rs := range rows {
			if _, ok := specs[s]; !ok {
				t.Fatalf("unknown section %q accepted", s)
			}
			for _, r := range rs {
				if r.obj["language"] != "go" {
					t.Fatalf("row in section %s has language %v", s, r.obj["language"])
				}
				if r.key == "" || r.canon == "" {
					t.Fatalf("empty key/canon for %v", r.obj)
				}
			}
		}
		// Accepted input must survive a whole merge and re-merge to the same bytes.
		dir := t.TempDir()
		if err := os.WriteFile(filepath.Join(dir, "labels-go.json"), []byte(raw), 0o644); err != nil {
			t.Fatal(err)
		}
		first, _, err := Merge(dir, Options{Private: private})
		if err != nil {
			return
		}
		if err := os.WriteFile(filepath.Join(dir, "labels-go.json"), first, 0o644); err != nil {
			t.Fatal(err)
		}
		second, _, err := Merge(dir, Options{Private: private})
		if err != nil || !bytes.Equal(first, second) {
			t.Fatalf("merge is not idempotent: err=%v\nfirst=%s\nsecond=%s", err, first, second)
		}
	})
}
