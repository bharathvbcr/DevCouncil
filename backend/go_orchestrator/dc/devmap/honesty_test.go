package devmap

import (
	"context"
	"strings"
	"testing"
)

// The fixtures below are the real kernel's wire form, captured from
// `devmap --json <cmd>` at schema 17 rather than written from the struct
// definitions. That distinction is the point of this file: the Go structs were
// written against a subset of the response, and a fixture derived from them
// would agree with the decoder about exactly the fields the decoder already
// knows. Only a fixture taken from the producer can show what is being dropped.

// TestAWalkThatStoppedEarlyIsNotAClean answer is the headline of this file.
//
// `walk_incomplete` is the kernel's field for "the producer of these items did
// not see everything" — set when a traversal hit its bound, when the ranking
// ran over a sample, and when the file behind the answer was recovered by
// pattern instead of parsed. It is distinct from the budget counters on
// purpose: `shown`/`hidden`/`total` describe a complete set that was trimmed,
// and a walk that withheld an unknown quantity cannot be expressed there
// without breaking `shown + hidden == total`.
//
// The fixture is a real `deps` answer for a file that parsed with one error
// range. Every budget counter in it says the answer is whole — hidden 0,
// truncated false, shown == total — and the index behind it is fresh. So every
// signal this package read before this test says "clean", and the one field
// that says otherwise is the one nothing decoded.
func TestAWalkThatStoppedEarlyIsNotClean(t *testing.T) {
	c := fake(t, map[string]string{
		"status": healthyStatus,
		"deps": `{"hidden":0,"items":[{"source_file":"broken.py","source_symbol":"broken.py",
		"target_file":"a.py","target_symbol":"a.py","edge_kind":"Imports","confidence":1.0}],
		"resolution":"Available","shown":1,"tokens_used":25,"total":1,"truncated":false,
		"walk_incomplete":"this file parsed with 1 error range(s); a call or import inside an error region is invisible to extraction, so this list is a lower bound"}`,
	})
	result, err := c.Deps(context.Background(), "broken.py")
	if err != nil {
		t.Fatal(err)
	}
	if result.Clean() {
		t.Error("an answer whose producer stopped early must not read as clean")
	}
	if result.WalkIncomplete == "" {
		t.Error("walk_incomplete was dropped by the decoder; the caller cannot see that the walk was partial")
	}
	// The reason has to reach the caller, not just the verdict. "incomplete" on
	// its own tells an agent nothing it can act on; "a call inside an error
	// region is invisible to extraction" tells it the list is a lower bound.
	if !strings.Contains(strings.Join(result.Degraded, " | "), "lower bound") {
		t.Errorf("the walk's own reason did not reach Degraded: %v", result.Degraded)
	}
}

// TestAnUnresolvableQueryIsNotAnEmptyAnswer is the same failure in its most
// consequential form, and unlike the rest of this file it was reproduced
// against the real binary: `devmap --json deps -- nosuch.py` answers
//
//	{"hidden":0,"items":[],"resolution":{"Unavailable":{"reason":"nosuch.py is
//	not indexed"}},"shown":0,"tokens_used":0,"total":0,"truncated":false}
//
// Nothing here decoded `resolution`, so that answer arrived as zero items,
// zero hidden, from a fresh index — which is the wire form of "this file has no
// dependencies". The kernel said it could not answer and the harness reported
// that it had.
func TestAnUnresolvableQueryIsNotAnEmptyAnswer(t *testing.T) {
	c := fake(t, map[string]string{
		"status": healthyStatus,
		"deps": `{"hidden":0,"items":[],"resolution":{"Unavailable":{"reason":"nosuch.py is not indexed"}},
		"shown":0,"tokens_used":0,"total":0,"truncated":false}`,
	})
	result, err := c.Deps(context.Background(), "nosuch.py")
	if err != nil {
		t.Fatal(err)
	}
	if result.Clean() {
		t.Error("an answer the kernel declined to resolve must not read as a clean empty result")
	}
	if result.Resolution.OK {
		t.Error("resolution was dropped by the decoder; an unavailable answer reads as an available one")
	}
	if !strings.Contains(strings.Join(result.Degraded, " | "), "not indexed") {
		t.Errorf("the kernel's own reason did not reach Degraded: %v", result.Degraded)
	}
}

// TestSuppressionIsReadFromEveryCounterThatCarriesIt.
//
// `hidden` was the only counter this package read. The kernel states
// suppression three ways over the same answer — `hidden`, `truncated`, and
// `total` against `shown` — and the Python client enforces
// `shown + hidden == total` and `truncated == (hidden > 0)` between them. A
// consumer that reads one of the three trusts the other two to agree without
// ever checking, so a producer that sets `truncated` and `total` but leaves
// `hidden` at zero is reported as a whole answer.
func TestSuppressionIsReadFromEveryCounterThatCarriesIt(t *testing.T) {
	for name, reply := range map[string]string{
		"truncated with no count": `{"hidden":0,"items":[{"file_path":"a.go","symbol_name":"F"}],
			"resolution":"Available","shown":1,"tokens_used":20,"total":1,"truncated":true}`,
		"total exceeds shown": `{"hidden":0,"items":[{"file_path":"a.go","symbol_name":"F"}],
			"resolution":"Available","shown":1,"tokens_used":20,"total":90,"truncated":false}`,
	} {
		t.Run(name, func(t *testing.T) {
			c := fake(t, map[string]string{"status": healthyStatus, "search": reply})
			result, err := c.Search(context.Background(), "F")
			if err != nil {
				t.Fatal(err)
			}
			if result.Clean() {
				t.Errorf("a capped answer read as complete: %+v", result)
			}
		})
	}
}

// TestADegradedIndexDegradesTheAnswersItGives.
//
// `degraded_reason` is the index's own statement that it is not what it should
// be. Available() refuses on it, but Available() is a startup question and the
// queries do not go through it — runQuery read `is_fresh`, `pending_count` and
// `quarantined_count` off the same status document and walked past the field
// that says the index is degraded outright.
func TestADegradedIndexDegradesTheAnswersItGives(t *testing.T) {
	c := fake(t, map[string]string{
		"status": `{"db_path":"x","generation_id":3,"node_count":1200,"edge_count":9000,
		"pending_count":0,"quarantined_count":0,"is_fresh":true,
		"degraded_reason":"the last build did not commit its analysis"}`,
		"search": `{"hidden":0,"items":[{"file_path":"a.go","symbol_name":"F"}],
		"resolution":"Available","shown":1,"tokens_used":20,"total":1,"truncated":false}`,
	})
	result, err := c.Search(context.Background(), "F")
	if err != nil {
		t.Fatal(err)
	}
	if result.Clean() {
		t.Error("answers from an index that reports itself degraded must not read as clean")
	}
	if !strings.Contains(strings.Join(result.Degraded, " | "), "did not commit its analysis") {
		t.Errorf("the index's own degradation reason did not reach the caller: %v", result.Degraded)
	}
}

// TestCoverageGapsReachTheCaller.
//
// `coverage_gaps` is what the index could not read: files discovery refused,
// files that failed to parse, files whose declarations were recovered by
// pattern, and the language-wide call- and import-blindness that makes an
// absent edge indistinguishable from an edge that does not exist. The kernel
// has emitted it on `status` since it began naming what it could not read, and
// no Go type had a field for it, so it stopped at the process boundary.
//
// It is deliberately not folded into a per-query Clean(): it is a property of
// the index rather than of one answer, it is non-zero on essentially every
// real repository, and a flag that is always on carries no information. What
// this test pins is that the numbers survive the decode, so a caller that wants
// to say "26 files were never parsed" can.
func TestCoverageGapsReachTheCaller(t *testing.T) {
	c := fake(t, map[string]string{
		"status": `{"db_path":"x","generation_id":3,"node_count":12,"edge_count":9,
		"pending_count":0,"quarantined_count":0,"is_fresh":true,"degraded_reason":null,
		"coverage_gaps":{"call_blind":{"paths":["x.cfm"],"shown":1,"total":1,"truncated":false},
		"discovery_refused":{"paths":[],"shown":0,"total":0,"truncated":false},
		"import_blind":{"paths":[],"shown":0,"total":26,"truncated":true},
		"not_parsed":{"paths":[],"shown":0,"total":0,"truncated":false},
		"parse_failed":{"paths":["y.go"],"shown":1,"total":3,"truncated":true},
		"pattern_recovered":{"paths":[],"shown":0,"total":0,"truncated":false}}}`,
	})
	status, err := c.Status(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if status.CoverageGaps.ImportBlind.Total != 26 {
		t.Errorf("import_blind total = %d, want 26 (the field did not survive the decode)",
			status.CoverageGaps.ImportBlind.Total)
	}
	if status.CoverageGaps.ParseFailed.Total != 3 {
		t.Errorf("parse_failed total = %d, want 3", status.CoverageGaps.ParseFailed.Total)
	}
	if !status.CoverageGaps.Any() {
		t.Error("a status naming files it could not read must report that it has gaps")
	}
	// The sample is capped by the producer. A caller must be able to tell the
	// one path it was shown from the three that exist, or it will report a
	// capped sample as the whole gap — the failure this whole file is about.
	if status.CoverageGaps.ParseFailed.Shown != 1 || !status.CoverageGaps.ParseFailed.Truncated {
		t.Errorf("parse_failed sample bounds did not survive: %+v", status.CoverageGaps.ParseFailed)
	}
}

// TestAnAbsentResolutionIsNotReadAsUnavailable guards the other direction of
// the fix. Every honesty field above is optional on the wire, and a decoder
// that reads absence as a positive claim would turn every answer from a
// producer that predates the field into a degraded one — trading a false clean
// for a false alarm. Absence is "not stated"; only an explicit Unavailable
// degrades.
func TestAnAbsentResolutionIsNotReadAsUnavailable(t *testing.T) {
	c := fake(t, map[string]string{
		"status": healthyStatus,
		"dead":   `{"hidden":0,"items":[{"file_path":"a.go","symbol_name":"Unused","confidence":0.9}]}`,
	})
	result, err := c.Dead(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if !result.Clean() {
		t.Errorf("an answer that states no qualification must stay clean: %v", result.Degraded)
	}
	if result.Resolution.Stated {
		t.Error("a resolution that was never on the wire must not be reported as stated")
	}
}
