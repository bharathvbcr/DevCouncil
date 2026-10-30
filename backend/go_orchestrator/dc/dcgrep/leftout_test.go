package dcgrep

import (
	"context"
	"fmt"
	"strings"
	"testing"
)

// The per-cause breakdown of the files an index left out crosses this boundary
// as a map of free strings. These tests hold the client to the contract the
// searcher keeps: the causes add up to the total, come from a closed set, are
// bounded, and name repository-relative paths.

// The searcher's own reply, from the real binary: a text file over the read
// bound and a file over the term cap, beside one ordinary file.
func TestTheRealSearchersLeftOutBreakdownIsAccepted(t *testing.T) {
	var terms strings.Builder
	for n := 0; n < 21000; n++ {
		// Letters only, so each is one term.
		word := make([]byte, 6)
		for digit, v := 0, n; digit < 6; digit, v = digit+1, v/26 {
			word[digit] = byte('a' + v%26)
		}
		terms.Write(word)
		terms.WriteByte(' ')
	}
	root := scratchRepo(t, map[string]string{
		"ok.go":        "package ok\nfunc Fine() {}\n",
		"big.txt":      strings.Repeat("word ", 500000),
		"nested/m.txt": terms.String(),
	})
	built, err := realClient(t, root).Index(context.Background(), IndexRequest{})
	if err != nil {
		t.Fatalf("the real searcher's own report must be accepted: %v", err)
	}
	if built.LexicalUnindexed != 2 {
		t.Fatalf("expected two files left out: %+v", built)
	}
	want := map[string]string{"oversized_text": "big.txt", "term_cap": "nested/m.txt"}
	if len(built.LexicalUnindexedByClass) != len(want) {
		t.Fatalf("the breakdown was not carried through: %+v", built.LexicalUnindexedByClass)
	}
	for class, path := range want {
		left, ok := built.LexicalUnindexedByClass[class]
		if !ok || left.Count != 1 || len(left.Sample) != 1 || left.Sample[0] != path {
			t.Fatalf("%s: want one file %s, got %+v", class, path, left)
		}
	}
}

func leftOutReply(unindexed int, classes string) string {
	return fmt.Sprintf(`#!/bin/sh
echo '{"ok":true,"engine":"tgrep-core","traversal_complete":true,
"files_seen":120,"files_indexed":10,"lexical_files":10,"lexical_unindexed":%d,
"lexical_vocabulary":"code-v1","lexical_unindexed_by_class":%s}'
`, unindexed, classes)
}

func TestALeftOutBreakdownThatContradictsItselfIsRefused(t *testing.T) {
	manyPaths := make([]string, MaxLeftOutSample+1)
	for i := range manyPaths {
		manyPaths[i] = fmt.Sprintf(`"f%d.txt"`, i)
	}
	cases := []struct {
		name      string
		unindexed int
		classes   string
	}{
		{"counts that do not add up to the total", 5, `{"term_cap":{"count":2,"sample":["a.txt"]}}`},
		{"counts that add up to more than the total", 2, `{"term_cap":{"count":2,"sample":[]},"unopenable":{"count":1,"sample":[]}}`},
		{"a cause the client does not know", 1, `{"bogus":{"count":1,"sample":["a.txt"]}}`},
		{"a negative count", 1, `{"term_cap":{"count":-1,"sample":[]},"unopenable":{"count":2,"sample":[]}}`},
		{"a sample over the searcher's bound", MaxLeftOutSample + 1,
			`{"term_cap":{"count":` + fmt.Sprint(MaxLeftOutSample+1) + `,"sample":[` + strings.Join(manyPaths, ",") + `]}}`},
		{"a sample longer than its own count", 3, `{"term_cap":{"count":3,"sample":["a","b","c","d"]}}`},
		// The attacker's observation: the one cause with no exact spelling
		// recorded an absolute path.
		{"an absolute path", 1, `{"unrepresentable_name":{"count":1,"sample":["/Users/someone/repo/caf�.rs"]}}`},
		{"a path that climbs out of the repository", 1, `{"unopenable":{"count":1,"sample":["../escape"]}}`},
		{"counts that overflow a sum", 9, `{"term_cap":{"count":9223372036854775807,"sample":[]},"unopenable":{"count":9223372036854775807,"sample":[]}}`},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			result, err := indexWith(t, leftOutReply(tc.unindexed, tc.classes))
			if err == nil {
				t.Fatalf("must be an error, got %+v", result)
			}
		})
	}
}

func TestALeftOutBreakdownThatAddsUpIsAccepted(t *testing.T) {
	result, err := indexWith(t, leftOutReply(3,
		`{"term_cap":{"count":2,"sample":["a.txt","dir/b.txt"]},"oversized_text":{"count":1,"sample":["c.txt"]}}`))
	if err != nil {
		t.Fatalf("a consistent breakdown must be accepted: %v", err)
	}
	if result.LexicalUnindexedByClass["term_cap"].Count != 2 {
		t.Fatalf("%+v", result.LexicalUnindexedByClass)
	}
	// And a searcher that predates the breakdown sends only the total.
	if _, err := indexWith(t, leftOutReply(3, `null`)); err != nil {
		t.Fatalf("a total with no breakdown is what an older searcher sends: %v", err)
	}
}
