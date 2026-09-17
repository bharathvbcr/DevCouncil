package dcgrep

import (
	"context"
	"testing"
	"time"
)

// The learned sparse tier crosses this boundary as two extra fields on an
// index report: which model produced the weights, and how much of the encoding
// the walk could not place. Both are things a caller acts on — the first
// decides whether a ranking is comparable to yesterday's, the second is the
// only signal that an encoding has gone stale — so both are checked here
// rather than passed through.

func indexWith(t *testing.T, body string) (*IndexResult, error) {
	t.Helper()
	return indexRequesting(t, body, IndexRequest{})
}

func indexRequesting(t *testing.T, body string, req IndexRequest) (*IndexResult, error) {
	t.Helper()
	client := New(fake(t, "fake.sh", body), t.TempDir())
	client.Timeout = 5 * time.Second
	return client.Index(context.Background(), req)
}

func TestAnIndexReportThatContradictsItselfAboutItsModelIsRefused(t *testing.T) {
	cases := []struct {
		name string
		body string
	}{
		// A model named while the vocabulary is the searcher's own BM25 space.
		// One of the two is wrong and there is no way to tell which, so a
		// caller must not be handed either.
		{"a model in the bm25 vocabulary", `#!/bin/sh
echo '{"ok":true,"engine":"tgrep-core","traversal_complete":true,
"files_seen":1,"files_indexed":1,"lexical_files":1,
"lexical_vocabulary":"code-v1","lexical_model":"some-encoder"}'
`},
		// And the reverse: a learned vocabulary with nothing to attribute it
		// to. The weights came from somewhere and the report will not say.
		{"a learned vocabulary with no model", `#!/bin/sh
echo '{"ok":true,"engine":"tgrep-core","traversal_complete":true,
"files_seen":1,"files_indexed":1,"lexical_files":1,
"lexical_vocabulary":"wordpiece-30522"}'
`},
		// Documents the walk never placed cannot be a negative number, and a
		// negative one here would read as "the encoding covered more than it
		// contained".
		{"a negative unmatched count", `#!/bin/sh
echo '{"ok":true,"engine":"tgrep-core","traversal_complete":true,
"files_seen":1,"files_indexed":1,"lexical_files":1,
"lexical_vocabulary":"code-v1","lexical_unmatched":-3}'
`},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			result, err := indexWith(t, tc.body)
			if err == nil {
				t.Fatalf("must be an error, got %+v", result)
			}
			if result != nil {
				t.Fatalf("an error must not also carry a result: %+v", result)
			}
		})
	}
}

func TestAConsistentLearnedIndexReportIsAccepted(t *testing.T) {
	result, err := indexWith(t, `#!/bin/sh
echo '{"ok":true,"engine":"tgrep-core","traversal_complete":true,
"files_seen":9,"files_indexed":9,"files_unindexed":0,
"lexical_files":7,"lexical_postings":400,"lexical_unindexed":2,
"lexical_vocabulary":"wordpiece-30522",
"lexical_model":"opensearch-project/opensearch-neural-sparse-encoding-doc-v2-mini",
"lexical_unmatched":4}'
`)
	if err != nil {
		t.Fatalf("a self-consistent report must be accepted: %v", err)
	}
	if result.LexicalVocabulary != "wordpiece-30522" {
		t.Fatalf("vocabulary was not carried through: %+v", result)
	}
	if result.LexicalModel == "" {
		t.Fatalf("model was not carried through: %+v", result)
	}
	// The stale-encoding signal. A caller that cannot read this cannot tell a
	// ranking over the whole repository from one over two thirds of it.
	if result.LexicalUnmatched != 4 {
		t.Fatalf("unmatched count was not carried through: %+v", result)
	}
}

func TestTheSparsePathReachesTheSearcherInTheRequest(t *testing.T) {
	// The encoding path is the one field of an index request that names a file
	// outside the repository. If it were dropped in transit the build would
	// quietly produce a BM25 index instead of the learned one asked for, and
	// every field of the reply would still look right.
	result, err := indexRequesting(t, `#!/bin/sh
request=$(cat)
case "$request" in
  *'"sparse":"/tmp/encoding.jsonl"'*)
    echo '{"ok":true,"engine":"tgrep-core","traversal_complete":true,
"files_seen":1,"files_indexed":1,"lexical_files":1,
"lexical_vocabulary":"wordpiece-30522","lexical_model":"m"}' ;;
  *) echo '{"ok":false,"error":"sparse path did not arrive: '"$request"'"}' ;;
esac
`, IndexRequest{Sparse: "/tmp/encoding.jsonl"})
	if err != nil {
		t.Fatalf("the sparse path must reach the searcher: %v", err)
	}
	if result.LexicalVocabulary != "wordpiece-30522" {
		t.Fatalf("%+v", result)
	}
}

func TestAnIndexRequestWithoutASparsePathDoesNotSendTheField(t *testing.T) {
	// `omitempty`, so a build that wants BM25 sends no key at all rather than
	// an empty string the searcher would have to interpret.
	result, err := indexWith(t, `#!/bin/sh
request=$(cat)
case "$request" in
  *sparse*) echo '{"ok":false,"error":"an empty sparse path was sent: '"$request"'"}' ;;
  *) echo '{"ok":true,"engine":"tgrep-core","traversal_complete":true,
"files_seen":1,"files_indexed":1,"lexical_files":1,
"lexical_vocabulary":"code-v1"}' ;;
esac
`)
	if err != nil {
		t.Fatalf("%v", err)
	}
	if result.LexicalModel != "" {
		t.Fatalf("a bm25 build must not name a model: %+v", result)
	}
}
