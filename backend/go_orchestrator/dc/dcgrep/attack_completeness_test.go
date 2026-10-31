package dcgrep

import (
	"strings"
	"testing"
)

// Adversarial review of the coverage fields. A test named "...IsRefused" that
// fails is a hostile report the client now believes.

// A trigram index that did not fill (traversal complete, no limit named, every
// unindexed count zero) holds every file the ranked index holds: a ranked file
// is read, UTF-8 text, and the trigram side admits exactly that. So
// lexical_files > files_indexed is only possible when the trigram index filled.
// The relaxed check accepts it from a report that says the trigram index did not.
func TestAttackIndexRankedMoreThanTrigramWithoutTheTrigramIndexFillingIsRefused(t *testing.T) {
	result, err := indexWith(t, `#!/bin/sh
echo '{"ok":true,"engine":"tgrep-core","traversal_complete":true,
"files_seen":10,"files_indexed":5,"files_unindexed":0,"lexical_files":10,
"lexical_vocabulary":"code-v1"}'
`)
	if err == nil {
		t.Fatalf("lexical_files 10 > files_indexed 5 with no postings limit is a contradiction: %+v", result)
	}
}

// A complete ranking over a walk that saw files but drew on none of them.
func TestAttackCompleteRankingThatDrewOnNothingOfWhatItSawIsRefused(t *testing.T) {
	result, err := rankWith(t, "#!/bin/sh\necho '{"+
		strings.Replace(rankedOne, `"files_indexed":1`, `"files_indexed":0`, 1)+
		`,"files_seen":100,"complete":true}'`+"\n")
	if err == nil {
		t.Logf("accepted (count=%d files_indexed=%d files_seen=%d complete=%v)",
			result.Count, result.FilesIndexed, result.FilesSeen, result.Complete)
	}
}

// A reason the client has never heard of is passed through as if it were one.
func TestAttackUnknownLimitReasonIsRefused(t *testing.T) {
	result, err := rankWith(t, "#!/bin/sh\necho '{"+rankedOne+
		`,"files_seen":5,"complete":false,"lexical_limit_reason":"bogus"}'`+"\n")
	if err == nil {
		t.Fatalf("an unknown limit reason was accepted: %q", result.Partial())
	}
}

// Type confusion on the field that decides everything.
func TestAttackCompleteAsAStringOrNullIsNeverComplete(t *testing.T) {
	for _, value := range []string{`"true"`, `1`, `"yes"`, `null`, `[]`} {
		result, err := rankWith(t, "#!/bin/sh\necho '{"+rankedOne+
			`,"files_seen":1,"complete":`+value+`}'`+"\n")
		if err == nil && result.Complete {
			t.Fatalf("complete=%s decoded as complete", value)
		}
	}
}

// Huge counts must not overflow the arithmetic anywhere or print nonsense.
func TestAttackHugeCountsDoNotPanicOrReadAsComplete(t *testing.T) {
	result, err := rankWith(t, "#!/bin/sh\necho '{"+
		strings.Replace(rankedOne, `"files_indexed":1`, `"files_indexed":9223372036854775807`, 1)+
		`,"files_seen":9223372036854775807,"complete":false,"lexical_limit_reason":"files"}'`+"\n")
	if err != nil {
		return
	}
	if result.Partial() == "" {
		t.Fatalf("a partial ranking produced no warning: %+v", result)
	}
}
