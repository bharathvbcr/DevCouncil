package dcgrep

import (
	"context"
	"strings"
	"testing"
	"time"
)

// A ranking over a prefix of the repository looks exactly like one over all of
// it unless the searcher says otherwise, and the searcher only learned to say
// so recently. These tests hold the client to the reading that cannot mislead:
// not known to be complete is incomplete, and a report that contradicts itself
// about coverage is refused rather than half-believed.

func rankWith(t *testing.T, body string) (*RankedResult, error) {
	t.Helper()
	client := New(fake(t, "fake.sh", body), t.TempDir())
	client.Timeout = 5 * time.Second
	return client.Rank(context.Background(), RankedRequest{Query: "json"})
}

const rankedOne = `"ok":true,"query":"json","count":1,` +
	`"files":[{"path":"src/a.go","score":1.5,"terms_present":1}],` +
	`"vocabulary":"code-v1","files_indexed":1,"index_postings":4,` +
	`"terms_unknown":0,"terms_total":1,"stale_files":0`

func TestARankingThatDoesNotSayItIsCompleteIsNotCompleteAndSaysSo(t *testing.T) {
	// Exactly what a searcher that predates the field sends.
	result, err := rankWith(t, "#!/bin/sh\necho '{"+rankedOne+"}'\n")
	if err != nil {
		t.Fatalf("an older searcher's ranking must still be usable: %v", err)
	}
	if result.Complete {
		t.Fatalf("a ranking with no `complete` was read as complete: %+v", result)
	}
	note := result.Partial()
	if note == "" {
		t.Fatalf("an unknown coverage must be reported, not passed in silence: %+v", result)
	}
	if !strings.Contains(note, "did not report") {
		t.Fatalf("the note must say the searcher did not vouch for the index: %q", note)
	}
}

func TestARankingThatSaysItIsCompleteHasNothingToWarnAbout(t *testing.T) {
	result, err := rankWith(t, "#!/bin/sh\necho '{"+rankedOne+
		`,"files_seen":1,"complete":true,"lexical_limit_reason":null}'`+"\n")
	if err != nil {
		t.Fatal(err)
	}
	if !result.Complete || result.Partial() != "" {
		t.Fatalf("a complete ranking was reported partial: %+v %q", result, result.Partial())
	}
}

func TestAPartialRankingNamesTheCeilingAndHowMuchItCovers(t *testing.T) {
	result, err := rankWith(t, "#!/bin/sh\necho '{"+rankedOne+
		`,"files_seen":2507,"complete":false,"lexical_limit_reason":"lexical_postings"}'`+"\n")
	if err != nil {
		t.Fatal(err)
	}
	if result.Complete {
		t.Fatalf("%+v", result)
	}
	if result.FilesSeen != 2507 || result.LexicalLimitReason != "lexical_postings" {
		t.Fatalf("the coverage fields were not carried through: %+v", result)
	}
	note := result.Partial()
	for _, want := range []string{"1 of the 2507 files", "lexical_postings"} {
		if !strings.Contains(note, want) {
			t.Fatalf("the note %q must contain %q", note, want)
		}
	}
}

func TestARankingThatContradictsItselfAboutCoverageIsRefused(t *testing.T) {
	// A `files_seen` of zero beside a nonzero `files_indexed` is what a searcher
	// that predates the field sends, and is accepted as incomplete; so the
	// contradictions below all carry a walk that reports a number.
	fiveDrawn := strings.Replace(rankedOne, `"files_indexed":1`, `"files_indexed":5`, 1)
	cases := []struct {
		name string
		body string
	}{
		{"more files ranked than seen",
			fiveDrawn + `,"files_seen":3,"complete":false,"lexical_limit_reason":"files"`},
		{"complete, yet a walk that saw nothing",
			rankedOne + `,"files_seen":0,"complete":true`},
		{"complete with a ceiling",
			rankedOne + `,"files_seen":1,"complete":true,"lexical_limit_reason":"duration"`},
		{"negative files seen",
			rankedOne + `,"files_seen":-1,"complete":false`},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			result, err := rankWith(t, "#!/bin/sh\necho '{"+tc.body+"}'\n")
			if err == nil {
				t.Fatalf("must be an error, got %+v", result)
			}
			if result != nil {
				t.Fatalf("an error must not also carry a result: %+v", result)
			}
		})
	}
}

func TestAnIndexWhoseRankedSideOutgrewTheTrigramSideIsAccepted(t *testing.T) {
	// The ordinary shape of a large repository now: the trigram index filled at
	// 1139 files and the ranked one carried on. This was refused as a
	// contradiction.
	result, err := indexWith(t, `#!/bin/sh
echo '{"ok":true,"engine":"tgrep-core","traversal_complete":false,
"limit_reason":"postings","files_seen":2507,"files_indexed":1139,
"files_unindexed":1368,"lexical_files":2400,"lexical_postings":900000,
"lexical_unindexed":50,"lexical_limit_reason":null,"lexical_vocabulary":"code-v1"}'
`)
	if err != nil {
		t.Fatalf("a ranked index larger than the trigram index must be accepted: %v", err)
	}
	if result.LexicalFiles != 2400 || result.FilesIndexed != 1139 {
		t.Fatalf("%+v", result)
	}
}

func TestAnIndexThatRankedMoreFilesThanItsWalkSawIsRefused(t *testing.T) {
	result, err := indexWith(t, `#!/bin/sh
echo '{"ok":true,"engine":"tgrep-core","traversal_complete":true,
"files_seen":10,"files_indexed":10,"lexical_files":11,"lexical_vocabulary":"code-v1"}'
`)
	if err == nil {
		t.Fatalf("an index cannot rank files its walk never reached: %+v", result)
	}
}

// complete means no ceiling, no unreadable directory entry and nothing
// admissible left out. The last two have no ceiling to name, so a report that
// pairs them with complete:true is the quietest way for this to be wrong.
func TestARankingThatIsCompleteDespiteAWalkErrorOrAFileLeftOutIsRefused(t *testing.T) {
	for _, tc := range []struct{ name, extra string }{
		{"walk errors", `,"files_seen":1,"complete":true,"walk_errors":2`},
		{"files left out", `,"files_seen":3,"complete":true,"lexical_unindexed":1`},
		{"negative walk errors", `,"files_seen":1,"complete":false,"walk_errors":-1`},
		{"negative left out", `,"files_seen":1,"complete":false,"lexical_unindexed":-4`},
	} {
		t.Run(tc.name, func(t *testing.T) {
			result, err := rankWith(t, "#!/bin/sh\necho '{"+rankedOne+tc.extra+"}'\n")
			if err == nil {
				t.Fatalf("must be an error, got %+v", result)
			}
		})
	}
}

func TestAnIncompleteRankingWithNoCeilingStillNamesWhatIsMissing(t *testing.T) {
	result, err := rankWith(t, "#!/bin/sh\necho '{"+rankedOne+
		`,"files_seen":9,"complete":false,"walk_errors":2,"lexical_unindexed":3}'`+"\n")
	if err != nil {
		t.Fatal(err)
	}
	if result.WalkErrors != 2 || result.LexicalUnindexed != 3 || result.Complete {
		t.Fatalf("the shortfall counts were not carried through: %+v", result)
	}
	note := result.Partial()
	for _, want := range []string{"2 directory entries", "3 readable-text files", "1 of the 9 files"} {
		if !strings.Contains(note, want) {
			t.Fatalf("the note %q must contain %q", note, want)
		}
	}
	if strings.Contains(note, "did not report") {
		t.Fatalf("a named shortfall is not an unknown one: %q", note)
	}
}

func TestEveryCeilingTheSearcherCanNameIsAccepted(t *testing.T) {
	for _, reason := range []string{"lexical_postings", "lexical_bytes", "files", "duration", "input_bytes"} {
		t.Run("rank "+reason, func(t *testing.T) {
			result, err := rankWith(t, "#!/bin/sh\necho '{"+rankedOne+
				`,"files_seen":5,"complete":false,"lexical_limit_reason":"`+reason+`"}'`+"\n")
			if err != nil {
				t.Fatalf("a ceiling the searcher names must be accepted: %v", err)
			}
			if !strings.Contains(result.Partial(), reason) {
				t.Fatalf("the note must name the ceiling: %q", result.Partial())
			}
		})
	}
}

func TestAnIndexNamingACeilingTheClientDoesNotKnowIsRefused(t *testing.T) {
	for _, tc := range []struct{ name, fields string }{
		{"unknown trigram reason", `"traversal_complete":false,"limit_reason":"bogus"`},
		// A reason of the ranked index's own cannot be the trigram index's.
		{"ranked reason in the trigram field", `"traversal_complete":false,"limit_reason":"lexical_bytes"`},
		{"unknown ranked reason", `"traversal_complete":true,"lexical_limit_reason":"bogus"`},
		// And the reverse: postings is the trigram index's ceiling alone.
		{"trigram reason in the ranked field", `"traversal_complete":true,"lexical_limit_reason":"postings"`},
	} {
		t.Run(tc.name, func(t *testing.T) {
			result, err := indexWith(t, "#!/bin/sh\necho '{\"ok\":true,\"engine\":\"tgrep-core\","+
				tc.fields+`,"files_seen":3,"files_indexed":3,"lexical_files":3,"lexical_vocabulary":"code-v1"}'`+"\n")
			if err == nil {
				t.Fatalf("must be an error, got %+v", result)
			}
		})
	}
}

func TestAnIndexNamingBothRealCeilingsIsAccepted(t *testing.T) {
	result, err := indexWith(t, `#!/bin/sh
echo '{"ok":true,"engine":"tgrep-core","traversal_complete":false,
"limit_reason":"postings","files_seen":9,"files_indexed":4,"lexical_files":6,
"lexical_limit_reason":"lexical_bytes","lexical_vocabulary":"code-v1"}'
`)
	if err != nil {
		t.Fatalf("the trigram and ranked ceilings are different fields with different sets: %v", err)
	}
	if result.LimitReason != "postings" || result.LexicalLimitReason != "lexical_bytes" {
		t.Fatalf("%+v", result)
	}
}
