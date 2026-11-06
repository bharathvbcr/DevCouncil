package devmap

import (
	"context"
	"strings"
	"testing"
)

const blastHead = "75bdac7d597816b990ff910a198e69784e0326f2"

// blastDoc is the shape `devmap blast --since <rev> --json` emits, with only
// what a decision rests on. The field names are copied from a real run; the
// live contract test below is what keeps them honest.
func blastDoc(unavailable, unattributed string) string {
	return `{
  "change": "HEAD~3..` + blastHead + `",
  "complete": false,
  "changed_files": [{"path": "rust/dc-grep/src/index.rs", "status": {"kind": "modified"}, "ranges": [{"start_line": 1, "end_line": 2, "deletion_only": false}]}],
  "files": [], "modules": [], "owners": [],
  "impacted": [{"distance": 1, "file_path": "rust/dc-grep/src/bin/dcgrep.rs", "qualified_name": "rust/dc-grep/src/bin/dcgrep.rs::rank"}],
  "seeds": [{"changed_lines": 3, "deletion_only": false, "end_line": 9, "file_path": "rust/dc-grep/src/index.rs", "qualified_name": "rust/dc-grep/src/index.rs::open", "start_line": 4}],
  "tests": [{"distance": 0, "path": "rust/dc-grep/src/index.rs"}, {"distance": 2, "path": "rust/dc-grep/tests/walk.rs"}],
  "unattributed": ` + unattributed + `,
  "unavailable": ` + unavailable + `
}`
}

const (
	limitIncomplete = `[{"depth": 3, "kind": "impact_incomplete", "reached": 238}]`
	rangeUnplaced   = `[{"end_line": 53, "path": "rust/dc-grep/src/lib.rs", "start_line": 53, "reason": {"kind": "outside_every_symbol", "lines_away": 18, "nearest_symbol": "rust/dc-grep/src/lib.rs::SCHEMA_VERSION"}}]`
)

func TestBlastDecodesEveryFieldADecisionRestsOn(t *testing.T) {
	b, err := DecodeBlast([]byte(blastDoc(`[{"kind": "change_unattributed", "lines": 16, "ranges": 3}, {"depth": 3, "kind": "impact_incomplete", "reached": 238}]`, rangeUnplaced)))
	if err != nil {
		t.Fatal(err)
	}
	if b.Complete || b.Head() != blastHead {
		t.Errorf("complete=%t head=%q", b.Complete, b.Head())
	}
	if len(b.Seeds) != 1 || b.Seeds[0].QualifiedName != "rust/dc-grep/src/index.rs::open" || b.Seeds[0].StartLine != 4 {
		t.Errorf("seeds = %+v", b.Seeds)
	}
	if len(b.Tests) != 2 || b.Tests[1].Path != "rust/dc-grep/tests/walk.rs" || b.Tests[1].Distance != 2 {
		t.Errorf("tests = %+v", b.Tests)
	}
	if len(b.ChangedFiles) != 1 || b.ChangedFiles[0].Status != "modified" || len(b.Impacted) != 1 {
		t.Errorf("changed=%+v impacted=%+v", b.ChangedFiles, b.Impacted)
	}
	if len(b.Unattributed) != 1 || b.Unattributed[0].Reason != "outside_every_symbol" ||
		b.Unattributed[0].NearestSymbol != "rust/dc-grep/src/lib.rs::SCHEMA_VERSION" {
		t.Errorf("unattributed = %+v", b.Unattributed)
	}
	if len(b.Unavailable) != 2 || b.Unavailable[0].Kind != "change_unattributed" || *b.Unavailable[0].Lines != 16 || *b.Unavailable[0].Ranges != 3 {
		t.Fatalf("unavailable = %+v", b.Unavailable)
	}
	if u := b.Unavailable[1]; u.Kind != "impact_incomplete" || *u.Depth != 3 || *u.Reached != 238 || !strings.Contains(string(u.Raw), "impact_incomplete") {
		t.Errorf("impact_incomplete = %+v", u)
	}
}

// Go fills an absent field with its zero value and says nothing, so a blast
// with no `unavailable` key would decode to "nothing is missing". Every key a
// decision rests on is required, and null is absent.
func TestBlastWithoutItsCompletenessEnvelopeIsRefused(t *testing.T) {
	for _, key := range []string{"change", "complete", "changed_files", "seeds", "impacted", "tests", "unattributed", "unavailable"} {
		for _, replace := range []string{"", "null"} {
			doc := blastDoc(limitIncomplete, "[]")
			needle := `"` + key + `":`
			at := strings.Index(doc, needle)
			if at < 0 {
				t.Fatalf("fixture lacks %s", key)
			}
			// Cut the key's whole value out of the document.
			end := at + len(needle)
			depth := 0
			for ; end < len(doc); end++ {
				c := doc[end]
				if c == '[' || c == '{' {
					depth++
				}
				if c == ']' || c == '}' {
					depth--
				}
				if depth == 0 && (c == ',' || c == '}') {
					break
				}
			}
			mutated := doc[:at] + replaceWith(key, replace) + doc[end:]
			if _, err := DecodeBlast([]byte(mutated)); err == nil {
				t.Errorf("a blast with %s %q decoded", key, replace)
			}
		}
	}
}

func replaceWith(key, replace string) string {
	if replace == "" {
		return `"x_` + key + `": 0`
	}
	return `"` + key + `": null`
}

func TestBlastRefusesAnythingThatIsNotAnIdentifiedRepositoryPath(t *testing.T) {
	for name, mutate := range map[string]func(string) string{
		"absolute test": func(d string) string { return strings.Replace(d, `"rust/dc-grep/tests/walk.rs"`, `"/etc/passwd"`, 1) },
		"dotdot test": func(d string) string {
			return strings.Replace(d, `"rust/dc-grep/tests/walk.rs"`, `"rust/../../x.rs"`, 1)
		},
		"backslash test": func(d string) string { return strings.Replace(d, `"rust/dc-grep/tests/walk.rs"`, `"rust\\x.rs"`, 1) },
		"empty seed":     func(d string) string { return strings.Replace(d, `"rust/dc-grep/src/index.rs::open"`, `""`, 1) },
		"empty changed": func(d string) string {
			return strings.Replace(d, `"path": "rust/dc-grep/src/index.rs", "status"`, `"path": "", "status"`, 1)
		},
		"no status": func(d string) string { return strings.Replace(d, `"status": {"kind": "modified"}`, `"status": {}`, 1) },
		"kindless limit": func(d string) string {
			return strings.Replace(d, `"kind": "impact_incomplete"`, `"what": "impact_incomplete"`, 1)
		},
		"not an object":     func(string) string { return `[1]` },
		"not json":          func(string) string { return `devmap: error` },
		"empty":             func(string) string { return `` },
		"negative distance": func(d string) string { return strings.Replace(d, `"distance": 2`, `"distance": -2`, 1) },
	} {
		if _, err := DecodeBlast([]byte(mutate(blastDoc(limitIncomplete, "[]")))); err == nil {
			t.Errorf("%s decoded", name)
		}
	}
	// Unattributed entries carry their own checks.
	if _, err := DecodeBlast([]byte(blastDoc(limitIncomplete, `[{"path": "a.rs", "start_line": 1, "end_line": 1}]`))); err == nil {
		t.Error("an unattributed range with no reason decoded")
	}
}

func TestAnUnknownLimitKindDecodesAndIsKeptVerbatim(t *testing.T) {
	b, err := DecodeBlast([]byte(blastDoc(`[{"kind": "a_kind_from_a_newer_devmap", "extra": 7}]`, "[]")))
	if err != nil {
		t.Fatalf("a producer upgrade must not fail the decode: %v", err)
	}
	if len(b.Unavailable) != 1 || b.Unavailable[0].Kind != "a_kind_from_a_newer_devmap" || !strings.Contains(string(b.Unavailable[0].Raw), `"extra": 7`) {
		t.Errorf("unavailable = %+v", b.Unavailable)
	}
}

func TestBlastHeadIsTheFullCommitAfterTheRangeOrEmpty(t *testing.T) {
	for change, want := range map[string]string{
		"HEAD~3.." + blastHead:             blastHead,
		blastHead + ".." + blastHead:       blastHead,
		"rust/dc-store/src/lib.rs:1-300":   "",
		"HEAD~3..HEAD":                     "",
		"a.." + strings.ToUpper(blastHead): "",
		"":                                 "",
	} {
		if got := (&Blast{Change: change}).Head(); got != want {
			t.Errorf("Head(%q) = %q, want %q", change, got, want)
		}
	}
}

func TestClientBlastAsksForTheRevisionAsOneArgumentAndDecodes(t *testing.T) {
	c := fake(t, map[string]string{"blast": blastDoc(limitIncomplete, "[]")})
	b, err := c.Blast(context.Background(), "origin/main")
	if err != nil {
		t.Fatal(err)
	}
	if len(b.Seeds) != 1 || b.Unavailable[0].Kind != "impact_incomplete" {
		t.Errorf("blast = %+v", b)
	}
}

func TestClientBlastRefusesARevisionThatCouldBeAFlag(t *testing.T) {
	c := fake(t, map[string]string{"blast": blastDoc(limitIncomplete, "[]")})
	for _, since := range []string{"", "-x", "--at", "a\nb"} {
		if _, err := c.Blast(context.Background(), since); err == nil {
			t.Errorf("Blast(%q) ran", since)
		}
	}
}

func TestClientBlastFailsRatherThanReturningAnEmptyAnswer(t *testing.T) {
	// A producer that answers with something else is an error, not an empty blast.
	c := fake(t, map[string]string{"blast": `{"seeds": []}`})
	if _, err := c.Blast(context.Background(), "HEAD~1"); err == nil {
		t.Error("a blast with no envelope was accepted")
	}
	c = fake(t, map[string]string{"blast": `devmap: no index`})
	if _, err := c.Blast(context.Background(), "HEAD~1"); err == nil {
		t.Error("unparseable output was accepted")
	}
}
