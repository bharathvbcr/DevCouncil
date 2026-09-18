package dcgrep

import (
	"context"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// These run against the binary the harness actually execs, not a fake.
//
// The adversarial tests beside them prove the client rejects a misbehaving
// reply; these prove the two halves of the boundary agree about the contract
// when both are real. A constant duplicated across a process boundary is a
// constant that drifts, and the drift is invisible from either side alone.

func realClient(t *testing.T, root string) *Client {
	t.Helper()
	return New(testsupport.DCGrep(t), root)
}

func scratchRepo(t *testing.T, files map[string]string) string {
	t.Helper()
	root := t.TempDir()
	for rel, body := range files {
		full := filepath.Join(root, rel)
		if err := os.MkdirAll(filepath.Dir(full), 0o750); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(full, []byte(body), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	return root
}

// TestTheHealthHandshakeMatchesTheRealBinary is the positive control for the
// probe: every negative case in the adversarial tests is only meaningful if the
// real binary passes.
func TestTheHealthHandshakeMatchesTheRealBinary(t *testing.T) {
	client := realClient(t, t.TempDir())
	if err := client.Available(context.Background()); err != nil {
		t.Fatalf("the real searcher must satisfy its own probe: %v", err)
	}
}

// TestMaxListResultsIsNotSilentlyClampedByTheSearcher pins a number that lives
// on both sides of a process boundary.
//
// The Go plane asks for MaxListResults when it wants every candidate file. The
// searcher clamps max_results to its own ceiling without saying so. If the Go
// constant were ever raised above the Rust one, this side would believe it had
// the whole tree while quietly receiving less — and every "file not found"
// built on that listing would be wrong in a way nothing reports.
func TestMaxListResultsIsNotSilentlyClampedByTheSearcher(t *testing.T) {
	// One more file than the limit, so truncation is forced and the searcher
	// must report the limit it actually applied.
	files := map[string]string{}
	const count = 12
	for i := range count {
		files[filepath.Join("src", "f"+itoa(i)+".txt")] = "x\n"
	}
	root := scratchRepo(t, files)
	client := realClient(t, root)

	// Asking for exactly MaxListResults must come back with that number as the
	// applied limit, not a smaller one the searcher substituted.
	listing, err := client.List(context.Background(), ListRequest{MaxResults: MaxListResults})
	if err != nil {
		t.Fatalf("list: %v", err)
	}
	if listing.Truncated {
		t.Fatalf("a %d-file tree must not truncate at %d: %+v", count, MaxListResults, listing)
	}

	// And the ceiling is real. This used to ask for MaxListResults+1000 over a
	// twelve-file tree and check the reported limit "if over.Truncated" — which
	// a twelve-file tree never is, so the check the comment called proof never
	// executed. Detecting a clamp behaviourally needs a tree larger than the
	// ceiling, and the ceiling is now 200,000. So the searcher reports it
	// instead, and the two constants are compared directly.
	raw, err := exec.Command(testsupport.DCGrep(t), "health").Output()
	if err != nil {
		t.Fatalf("health: %v", err)
	}
	var health struct {
		Limits struct {
			MaxResults     int `json:"max_results"`
			MaxListResults int `json:"max_list_results"`
		} `json:"limits"`
	}
	if err := json.Unmarshal(raw, &health); err != nil {
		t.Fatalf("unparseable health reply: %v", err)
	}
	if health.Limits.MaxListResults != MaxListResults {
		t.Fatalf("the searcher lists up to %d while this side asks for %d; a "+
			"caller wanting every candidate would receive %d of them and be told nothing",
			health.Limits.MaxListResults, MaxListResults,
			min(health.Limits.MaxListResults, MaxListResults))
	}
	// The listing ceiling must stay above the search ceiling. If they were
	// merged again, this test would still pass on the constant above while the
	// enumeration silently became a sample.
	if health.Limits.MaxListResults <= health.Limits.MaxResults {
		t.Fatalf("the searcher's listing ceiling (%d) is not above its search "+
			"ceiling (%d); enumeration has been clamped to sampling again",
			health.Limits.MaxListResults, health.Limits.MaxResults)
	}
}

// TestAnOversizedRequestIsRefusedByTheBinary covers the inbound bound.
//
// Every other boundary here caps what it will *read back* from a child. This
// one also caps what the child will accept, because an unbounded
// read_to_string turned a 64 MiB request into a 75 MiB resident set before
// anything inspected it — and the pattern inside that request comes from a
// model.
func TestAnOversizedRequestIsRefusedByTheBinary(t *testing.T) {
	root := scratchRepo(t, map[string]string{"a.txt": "needle\n"})
	client := realClient(t, root)

	// Over the searcher's own pattern limit and its request limit at once; the
	// point is that neither is reached by allocating first.
	_, err := client.Search(context.Background(), Request{
		Pattern: strings.Repeat("a", 4*1024*1024),
	})
	if err == nil {
		t.Fatal("an oversized request must be refused")
	}
	if !strings.Contains(err.Error(), "limit") {
		t.Fatalf("the refusal must name the bound, got %v", err)
	}
}

// TestTheListingAndTheSearchAgreeAcrossTheBoundary re-asserts the engine's own
// invariant through the client, so a future change to either wire shape that
// broke the correspondence would be caught here rather than in a turn.
func TestTheListingAndTheSearchAgreeAcrossTheBoundary(t *testing.T) {
	root := scratchRepo(t, map[string]string{
		".gitignore":        "dist/\n",
		"src/a.go":          "needle\n",
		"src/deep/b.go":     "needle\n",
		"dist/generated.go": "needle\n",
		".hidden/c.go":      "needle\n",
	})
	client := realClient(t, root)

	for _, includeIgnored := range []bool{false, true} {
		listing, err := client.List(context.Background(),
			ListRequest{MaxResults: MaxListResults, IncludeIgnored: includeIgnored})
		if err != nil {
			t.Fatalf("list: %v", err)
		}
		result, err := client.Search(context.Background(),
			Request{Pattern: "needle", MaxResults: 1000, IncludeIgnored: includeIgnored})
		if err != nil {
			t.Fatalf("search: %v", err)
		}

		listed := map[string]bool{}
		for _, path := range listing.Paths {
			listed[path] = true
		}
		for _, hit := range result.Matches {
			if !listed[hit.Path] {
				t.Fatalf("include_ignored=%v: search opened %q, which the listing does not name",
					includeIgnored, hit.Path)
			}
		}
		if listing.IgnoreRulesApplied != result.IgnoreRulesApplied {
			t.Fatalf("include_ignored=%v: the two operations disagree about the mode they ran in",
				includeIgnored)
		}
	}
}

// TestTheLearnedTierWorksEndToEndAcrossTheBoundary drives the whole learned
// path with both halves real: a hand-written encoding standing in for a model
// run, the Rust binary importing it, and this client reading the ranking back.
//
// Hand-written on purpose. If a fixture a test can type is enough to produce a
// learned ranking, then nothing on the search path loads a model — which is
// the property that keeps this boundary one fork/exec of a static binary
// rather than a Python runtime the orchestrator has to own.
func TestTheLearnedTierWorksEndToEndAcrossTheBoundary(t *testing.T) {
	root := scratchRepo(t, map[string]string{
		"src/alpha.go": "package alpha\n",
		"src/beta.go":  "package beta\n",
	})
	// The encoding lives outside the repository, where a producer writes it
	// and where the walk cannot index it into the thing it describes.
	encoding := filepath.Join(t.TempDir(), "sparse.jsonl")
	// Ids are positions in `vocab`. The parity pairs are what the searcher
	// replays through its own tokeniser before it will publish anything; these
	// are the ids it genuinely produces, so the build is expected to succeed.
	body := strings.Join([]string{
		`{"schema":1,"model":"interop-fixture","vocabulary":"wordpiece-30522",` +
			`"vocab":["[UNK]","parse","json","server"],` +
			`"query_weights":[0.0,2.0,3.0,1.0],` +
			`"parity":[{"text":"parse","ids":[1]},{"text":"json","ids":[2]}]}`,
		`{"path":"src/alpha.go","total_terms":12,"terms":[[2,0.90]]}`,
		`{"path":"src/beta.go","total_terms":12,"terms":[[2,0.10]]}`,
		"",
	}, "\n")
	if err := os.WriteFile(encoding, []byte(body), 0o600); err != nil {
		t.Fatal(err)
	}

	client := realClient(t, root)
	built, err := client.Index(context.Background(), IndexRequest{Sparse: encoding})
	if err != nil {
		t.Fatalf("the learned build must succeed: %v", err)
	}
	if built.LexicalVocabulary != "wordpiece-30522" {
		t.Fatalf("the build did not publish a learned index: %+v", built)
	}
	if built.LexicalModel != "interop-fixture" {
		t.Fatalf("the build did not attribute the weights: %+v", built)
	}

	ranked, err := client.Rank(context.Background(), RankedRequest{Query: "json"})
	if err != nil {
		t.Fatalf("rank: %v", err)
	}
	if ranked.Vocabulary != "wordpiece-30522" {
		t.Fatalf("the ranking came from the wrong term space: %+v", ranked)
	}
	// alpha outweighs beta nine to one in the encoding and nowhere else — the
	// two files' text is the same length and shares no word with the query.
	if len(ranked.Files) != 2 || ranked.Files[0].Path != "src/alpha.go" {
		t.Fatalf("the encoder's weights did not decide the order: %+v", ranked.Files)
	}

	// And a tokeniser that disagrees must refuse rather than publish. The
	// claim below is false: this build resolves "parse" to 1, not 3.
	bad := strings.Replace(body, `{"text":"parse","ids":[1]}`, `{"text":"parse","ids":[3]}`, 1)
	if err := os.WriteFile(encoding, []byte(bad), 0o600); err != nil {
		t.Fatal(err)
	}
	result, err := client.Index(context.Background(), IndexRequest{Sparse: encoding})
	if err == nil {
		t.Fatalf("a tokeniser disagreement must refuse the build, got %+v", result)
	}
	if !strings.Contains(err.Error(), "disagrees with the model") {
		t.Fatalf("the refusal must name the cause: %v", err)
	}

	// The refused build published nothing, so the previous index is still the
	// one that answers. A half-published slot would show up here as a ranking
	// that changed without a successful build.
	again, err := client.Rank(context.Background(), RankedRequest{Query: "json"})
	if err != nil {
		t.Fatalf("the previous index must still answer: %v", err)
	}
	if len(again.Files) != 2 || again.Files[0].Path != "src/alpha.go" {
		t.Fatalf("a refused build disturbed the published index: %+v", again.Files)
	}
}

func itoa(n int) string {
	if n == 0 {
		return "0"
	}
	var digits []byte
	for n > 0 {
		digits = append([]byte{byte('0' + n%10)}, digits...)
		n /= 10
	}
	return string(digits)
}
