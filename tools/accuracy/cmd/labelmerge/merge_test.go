package main

import (
	"bytes"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"testing"
)

const storeDir = "testdata/store"

// write puts files (name -> content) in a fresh directory.
func write(t *testing.T, files map[string]string) string {
	t.Helper()
	dir := t.TempDir()
	for name, content := range files {
		if err := os.WriteFile(filepath.Join(dir, name), []byte(content), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	return dir
}

func TestGoldenMerge(t *testing.T) {
	got, counts, err := Merge(storeDir, Options{Private: true})
	if err != nil {
		t.Fatal(err)
	}
	golden := filepath.Join("testdata", "merged.golden.json")
	want, err := os.ReadFile(golden)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(got, want) {
		t.Fatalf("merged labels differ from %s:\n%s", golden, got)
	}
	// The identical Target01 edge and recall site in r2 are one entry.
	if counts["go"] != 9 || counts["python"] != 1 {
		t.Fatalf("counts = %v", counts)
	}
	again, _, err := Merge(storeDir, Options{Private: true})
	if err != nil || !bytes.Equal(again, got) {
		t.Fatalf("a second merge differs or failed: %v", err)
	}
}

func TestConflictingSiteNamesBothFiles(t *testing.T) {
	dir := write(t, map[string]string{
		"labels-go.json":    `{"edges":[{"language":"go","caller":"a::F","callee":"a::G","correct":true}]}`,
		"labels-go-r2.json": `{"edges":[{"language":"go","caller":"a::F","callee":"a::G","correct":false}]}`,
	})
	_, _, err := Merge(dir, Options{})
	if err == nil {
		t.Fatal("a site labelled differently in two rounds was merged")
	}
	for _, want := range []string{"labels-go.json", "labels-go-r2.json", "a::F"} {
		if !strings.Contains(err.Error(), want) {
			t.Errorf("error %q does not name %q", err, want)
		}
	}
}

func TestIdenticalDuplicateAcrossRoundsIsKeptOnce(t *testing.T) {
	site := `{"language":"go","construct":"plain","caller":"a::F","name":"G","truth":"a::G"}`
	dir := write(t, map[string]string{
		"labels-go.json":    `{"recall":[` + site + `]}`,
		"labels-go-r3.json": `{"recall":[` + site + `]}`,
	})
	out, counts, err := Merge(dir, Options{})
	if err != nil {
		t.Fatal(err)
	}
	if counts["go"] != 1 || strings.Count(string(out), `"caller": "a::F"`) != 1 {
		t.Fatalf("counts=%v\n%s", counts, out)
	}
}

func TestMalformedFilesAreRefusedNamingTheFile(t *testing.T) {
	cases := map[string]string{
		"not json":              `{"edges": [`,
		"not an object":         `[]`,
		"trailing data":         `{} {}`,
		"unknown key":           `{"edgez": []}`,
		"section not an array":  `{"edges": {}}`,
		"missing string field":  `{"edges":[{"language":"go","caller":"a::F","correct":true}]}`,
		"missing bool field":    `{"edges":[{"language":"go","caller":"a::F","callee":"a::G"}]}`,
		"oracle without uses":   `{"oracle":[{"language":"go","covered_callers":[]}]}`,
		"other language's rows": `{"edges":[{"language":"python","caller":"a::F","callee":"a::G","correct":true}]}`,
	}
	for name, content := range cases {
		dir := write(t, map[string]string{"labels-go.json": content})
		_, _, err := Merge(dir, Options{})
		if err == nil {
			t.Errorf("%s: merged", name)
			continue
		}
		if !strings.Contains(err.Error(), "labels-go.json") {
			t.Errorf("%s: error %q does not name the file", name, err)
		}
	}
}

func TestPrivateSymbolHashShapeMatchesTheRunner(t *testing.T) {
	dead := func(extra string) string {
		return `{"dead":[{"language":"go","id":"a::F","truly_dead":true` + extra + `}]}`
	}
	bad := []string{
		`,"symbol_hash":"h:0123456789AB","category":"rg_declaration_only"`,
		`,"symbol_hash":"h:0123456789a","category":"rg_declaration_only"`,
		`,"symbol_hash":"0123456789ab","category":"rg_declaration_only"`,
		`,"symbol_hash":7,"category":"rg_declaration_only"`,
	}
	for _, extra := range bad {
		dir := write(t, map[string]string{"labels-go.json": dead(extra)})
		if _, _, err := Merge(dir, Options{}); err == nil || !strings.Contains(err.Error(), "labels-go.json") {
			t.Errorf("%s: err = %v", extra, err)
		}
	}
	// Private: hash and a known category are both required.
	for _, extra := range []string{``, `,"symbol_hash":"h:0123456789ab"`, `,"symbol_hash":"h:0123456789ab","category":"nothing imports it"`} {
		dir := write(t, map[string]string{"labels-go.json": dead(extra)})
		if _, _, err := Merge(dir, Options{Private: true}); err == nil {
			t.Errorf("private %q merged", extra)
		}
	}
	// A public corpus's label needs neither.
	dir := write(t, map[string]string{"labels-go.json": dead(``)})
	if _, _, err := Merge(dir, Options{}); err != nil {
		t.Errorf("public label refused: %v", err)
	}
}

func TestEmptyAndUnusableDirectoriesAreRefused(t *testing.T) {
	if _, _, err := Merge(t.TempDir(), Options{}); err == nil || !strings.Contains(err.Error(), "no labels-") {
		t.Errorf("empty dir: %v", err)
	}
	if _, _, err := Merge(filepath.Join(t.TempDir(), "absent"), Options{}); err == nil {
		t.Error("a missing dir merged")
	}
	// Unrelated files are ignored; a misnamed label file is not.
	dir := write(t, map[string]string{".DS_Store": "x", "notes.txt": "x"})
	if _, _, err := Merge(dir, Options{}); err == nil {
		t.Error("a dir with no label files merged")
	}
	dir = write(t, map[string]string{"labels-go.json": `{}`, "labels-go-round2.json": `{}`})
	if _, _, err := Merge(dir, Options{}); err == nil || !strings.Contains(err.Error(), "labels-go-round2.json") {
		t.Errorf("misnamed round: %v", err)
	}
}

func TestEmptyObjectFilesMergeToEmptyArrays(t *testing.T) {
	dir := write(t, map[string]string{"labels-go.json": `{}`})
	out, counts, err := Merge(dir, Options{})
	if err != nil || len(counts) != 0 {
		t.Fatalf("err=%v counts=%v", err, counts)
	}
	want := "{\n  \"dead\": [],\n  \"edges\": [],\n  \"oracle\": [],\n  \"recall\": []\n}\n"
	if string(out) != want {
		t.Fatalf("got %q", out)
	}
}

func TestRunRefusesToWriteOverAnInput(t *testing.T) {
	dir := write(t, map[string]string{"labels-go.json": `{}`})
	input := filepath.Join(dir, "labels-go.json")
	var stdout, stderr bytes.Buffer
	if code := run([]string{"-dir", dir, "-o", input}, &stdout, &stderr); code != 1 {
		t.Fatalf("exit %d, stderr %q", code, stderr.String())
	}
	if !strings.Contains(stderr.String(), "refusing to write over the input") {
		t.Errorf("stderr %q", stderr.String())
	}
	if raw, _ := os.ReadFile(input); string(raw) != `{}` {
		t.Errorf("the input was modified: %q", raw)
	}
	// A symlink to the input is the same file.
	link := filepath.Join(t.TempDir(), "out.json")
	if err := os.Symlink(input, link); err != nil {
		t.Fatal(err)
	}
	stderr.Reset()
	if code := run([]string{"-dir", dir, "-o", link}, &stdout, &stderr); code != 1 {
		t.Fatalf("symlink: exit %d", code)
	}
	// A fresh output path is written.
	out := filepath.Join(t.TempDir(), "labels.json")
	if code := run([]string{"-dir", dir, "-o", out}, &stdout, &stderr); code != 0 {
		t.Fatalf("exit %d, stderr %q", code, stderr.String())
	}
	if _, err := os.Stat(out); err != nil {
		t.Error(err)
	}
}

func TestRunUsageErrors(t *testing.T) {
	var stdout, stderr bytes.Buffer
	if code := run(nil, &stdout, &stderr); code != 2 {
		t.Errorf("no -dir: exit %d", code)
	}
	if code := run([]string{"-dir", "x", "extra"}, &stdout, &stderr); code != 2 {
		t.Errorf("stray arg: exit %d", code)
	}
}

// The runner owns the hash shape and the category list; this tool restates
// them so it can refuse early. Read the Rust source so they cannot drift.
func TestIdentityRulesMatchTheRunner(t *testing.T) {
	raw, err := os.ReadFile(filepath.Join("..", "..", "..", "..", "rust", "devmap-analyze", "src", "record_identity.rs"))
	if err != nil {
		t.Fatal(err)
	}
	src := string(raw)
	list := regexp.MustCompile(`(?s)EVIDENCE_CATEGORIES: &\[&str\] = &\[(.*?)\];`).FindStringSubmatch(src)
	if list == nil {
		t.Fatal("EVIDENCE_CATEGORIES not found in record_identity.rs")
	}
	var rust []string
	for _, m := range regexp.MustCompile(`"([a-z_]+)"`).FindAllStringSubmatch(list[1], -1) {
		rust = append(rust, m[1])
	}
	if strings.Join(rust, ",") != strings.Join(evidenceCategories, ",") {
		t.Errorf("categories drifted:\n rust %v\n go   %v", rust, evidenceCategories)
	}
	if !strings.Contains(src, `hex.len() == 12`) || !strings.Contains(src, `strip_prefix("h:")`) {
		t.Error("record_identity.rs no longer checks `h:` + 12 hex digits; update identityHash")
	}
}
