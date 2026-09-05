package repomap

import (
	"encoding/json"
	"fmt"
	"io"
	"math"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

// The graph is a file. That is the whole of this file's subject.
//
// stress_test.go next door treats the artifact's *content* as hostile — the
// shapes a producer can put inside a well-formed document. These are the
// failures that come from it being a file rather than a value: it has a size
// this process did not choose, it outlives the index it was rendered from, it
// can be rewritten underneath a reader, and the producer that writes it is a
// binary from another repository whose vocabulary can move between releases.
//
// Each check here asserts the same property in a different place: an answer
// this package could not verify must never be reported the way it reports one
// it verified and found sound.

// writeGraph puts a document at a fresh path and returns it.
func writeGraph(t *testing.T, doc any) string {
	t.Helper()
	raw, err := json.Marshal(doc)
	if err != nil {
		t.Fatal(err)
	}
	p := filepath.Join(t.TempDir(), "code_graph.json")
	if err := os.WriteFile(p, raw, 0o644); err != nil {
		t.Fatal(err)
	}
	return p
}

// oneNode is the smallest document Load accepts, so a test can vary exactly the
// field it is about.
func oneNode() []map[string]any {
	return []map[string]any{{"id": "a/x.go", "kind": "file", "path": "a/x.go", "area": "a"}}
}

// mentions fails unless every fragment appears in the joined lines.
func mentions(t *testing.T, what string, lines []string, fragments ...string) {
	t.Helper()
	joined := strings.Join(lines, " | ")
	for _, f := range fragments {
		if !strings.Contains(joined, f) {
			t.Errorf("%s does not mention %q: %v", what, f, lines)
		}
	}
}

// TestAnArtifactLargerThanTheBoundIsRefusedRatherThanRead.
//
// Every other payload this harness takes from another process is bounded: the
// reply on devmap's stdout at 64 MiB, its stderr at 4 MiB, the marker scan to a
// 64 KiB window, the retained notices, the preserved copies. This read was not.
// It is the largest of them — 20 MiB on this repository and growing with the
// tree — it is a file some other process is free to rewrite, and reading it
// costs 3.35x its size in transient allocation, measured by
// BenchmarkLoadRealisticGraph next door.
//
// The failure that bound prevents is not subtle: an artifact that is wrong
// about its own size takes the harness's memory at session start, which is the
// one moment there is nothing else to fall back on.
func TestAnArtifactLargerThanTheBoundIsRefusedRatherThanRead(t *testing.T) {
	p := filepath.Join(t.TempDir(), "code_graph.json")
	f, err := os.Create(p)
	if err != nil {
		t.Fatal(err)
	}
	// Sparse: the bytes are never written, so this costs no disk and no time.
	// What is under test is the decision made about the size, which is taken
	// before anything is read.
	if err := f.Truncate(MaxGraphBytes + 1); err != nil {
		f.Close()
		t.Fatal(err)
	}
	if err := f.Close(); err != nil {
		t.Fatal(err)
	}

	_, err = Load(p)
	if err == nil {
		t.Fatal("an artifact past the bound must be refused, not loaded")
	}
	// The error has to name the bound. Refusing with the parse failure that
	// follows from reading it would send an operator to look at the producer's
	// JSON, which is not what went wrong.
	for _, want := range []string{"larger than", "bound"} {
		if !strings.Contains(err.Error(), want) {
			t.Errorf("the refusal must name the bound (%q), got: %v", want, err)
		}
	}
}

// TestABoundedReadStopsASourceThatUnderstatesItself.
//
// The size check above is taken from the file's own metadata, and a file being
// rewritten by another process is exactly the case where that metadata is a
// stale answer — the producer's write is in flight and the file is longer than
// the stat that preceded the read. A bound that trusts stat is a bound that
// does not hold precisely when it is needed, so the read is limited as well as
// the stat, and this drives the second one on a source whose stat cannot help:
// a pipe reports size zero and delivers as much as its writer chooses.
func TestABoundedReadStopsASourceThatUnderstatesItself(t *testing.T) {
	r, w, err := os.Pipe()
	if err != nil {
		t.Fatal(err)
	}
	defer r.Close()

	const bound = 4 << 10
	// The writer runs until the reader stops taking bytes; its own write error
	// is the expected end of this test rather than a failure of it.
	done := make(chan struct{})
	go func() {
		defer close(done)
		defer w.Close()
		chunk := make([]byte, 1<<10)
		for i := range chunk {
			chunk[i] = ' '
		}
		for written := 0; written < 8*bound; written += len(chunk) {
			if _, err := w.Write(chunk); err != nil {
				return
			}
		}
	}()

	raw, err := readBounded(r, bound)
	if err == nil {
		t.Fatalf("a source past the bound must be refused; it returned %d bytes", len(raw))
	}
	if !strings.Contains(err.Error(), "bound") {
		t.Errorf("the refusal must name the bound, got: %v", err)
	}
	if int64(len(raw)) > bound {
		t.Errorf("the refusal still took %d bytes past a %d byte bound", len(raw), bound)
	}
	w.Close()
	<-done
}

// TestASourceWithinTheBoundIsReadWhole. The guard above must not be a guard
// that also stops the ordinary case: a stream whose stat says nothing and whose
// content fits has to arrive complete, or the bound would turn every artifact
// into a truncated one.
func TestASourceWithinTheBoundIsReadWhole(t *testing.T) {
	r, w, err := os.Pipe()
	if err != nil {
		t.Fatal(err)
	}
	defer r.Close()

	const bound = 4 << 10
	payload := strings.Repeat("x", bound) // exactly the bound, not one byte under
	go func() {
		defer w.Close()
		if _, err := io.WriteString(w, payload); err != nil {
			return
		}
	}()

	raw, err := readBounded(r, bound)
	if err != nil {
		t.Fatalf("a source of exactly the bound is within it: %v", err)
	}
	if string(raw) != payload {
		t.Fatalf("read %d bytes of a %d byte source", len(raw), len(payload))
	}
}

// TestAGraphThatDeclaresNoSchemaIsNotReadAsTheOneThisBuildKnows.
//
// This is the rule the rest of the package is built on, applied to the one
// field that decides whether any of the others mean what this build thinks. The
// producer emits `schema_version: 2` today. If it moves or renames that key —
// the drift this package's own doc comments are about — Go's decoder leaves the
// field at zero and reports no error, and the guard at the top of Degraded
// tested `!= 0` before comparing. So the document that proves nothing about its
// schema produced exactly the same verdict as the one that declared the
// supported version: silence.
//
// Absent is not zero and it is not two. It is unknown, and unknown has to say
// so, because everything downstream — an area, a coupling, a neighbour verdict
// — is read out of fields whose meaning that number fixes.
func TestAGraphThatDeclaresNoSchemaIsNotReadAsTheOneThisBuildKnows(t *testing.T) {
	m, err := Load(writeGraph(t, map[string]any{"nodes": oneNode(), "edges": []any{}}))
	if err != nil {
		t.Fatal(err)
	}
	if len(m.Degraded()) == 0 {
		t.Fatal("a graph that declares no schema version was reported as sound; " +
			"an unversioned document must not read as the supported one")
	}
	mentions(t, "the unversioned graph's report", m.Degraded(), "declares no schema version")
}

// TestANewerSchemaIsNotDescribedAsAnOlderOne.
//
// Both directions were one sentence, and they are not one failure. Reading an
// older graph, this build looks for fields that have not been written yet and
// finds them absent, which is what the message described. Reading a *newer*
// one, the fields are there and this build's understanding of them is the stale
// half: a value it matches on by name — `extracted`, `calls` — may have been
// split, renamed, or given a meaning it no longer has, and the map is then
// confidently wrong rather than visibly short.
//
// The direction is the operator's remedy. One says rebuild the artifact; the
// other says this binary is behind the producer.
func TestANewerSchemaIsNotDescribedAsAnOlderOne(t *testing.T) {
	older, err := Load(writeGraph(t, map[string]any{
		"schema_version": SupportedSchema - 1, "nodes": oneNode(), "edges": []any{}}))
	if err != nil {
		t.Fatal(err)
	}
	newer, err := Load(writeGraph(t, map[string]any{
		"schema_version": SupportedSchema + 1, "nodes": oneNode(), "edges": []any{}}))
	if err != nil {
		t.Fatal(err)
	}
	if len(older.Degraded()) == 0 || len(newer.Degraded()) == 0 {
		t.Fatal("both a newer and an older schema must be reported")
	}
	mentions(t, "the older graph's report", older.Degraded(), "older")
	mentions(t, "the newer graph's report", newer.Degraded(), "newer")
	if strings.Join(older.Degraded(), "|") == strings.Join(newer.Degraded(), "|") {
		t.Errorf("a newer and an older schema produced the same sentence, so the report "+
			"cannot tell an operator which side is behind: %v", newer.Degraded())
	}
}

// TestStalenessIsDetectedRatherThanAsserted.
//
// DisagreementsWith carries the package's argument for why the artifact needs a
// provenance stamp at all, and nothing asserted that the comparison fires. This
// drives the three answers it has to keep apart, at the boundary between them.
func TestStalenessIsDetectedRatherThanAsserted(t *testing.T) {
	stamped := func(generation, nodes int) *Map {
		t.Helper()
		ns := make([]map[string]any, 0, nodes)
		for i := 0; i < nodes; i++ {
			ns = append(ns, map[string]any{
				"id": fmt.Sprintf("a/x%d.go", i), "kind": "file",
				"path": fmt.Sprintf("a/x%d.go", i), "area": "a"})
		}
		m, err := Load(writeGraph(t, map[string]any{
			"schema_version": SupportedSchema, "nodes": ns, "edges": []any{},
			"meta": map[string]any{"devmap_rust": map[string]any{"generation_id": generation}},
		}))
		if err != nil {
			t.Fatal(err)
		}
		return m
	}

	t.Run("an artifact from the generation the index holds is current", func(t *testing.T) {
		if notes := stamped(4, 3).DisagreementsWith(4, 3); len(notes) != 0 {
			t.Fatalf("an artifact that matches its index must be clean: %v", notes)
		}
	})

	t.Run("an artifact from an earlier generation is stale", func(t *testing.T) {
		notes := stamped(2, 3).DisagreementsWith(4, 3)
		if len(notes) == 0 {
			t.Fatal("an artifact two generations behind the index was reported as current")
		}
		mentions(t, "the staleness report", notes, "generation 2", "stands at 4")
	})

	t.Run("an artifact holding different nodes did not come from that index", func(t *testing.T) {
		notes := stamped(4, 3).DisagreementsWith(4, 9)
		if len(notes) == 0 {
			t.Fatal("an artifact with a third of the index's nodes was reported as current")
		}
		mentions(t, "the node-count report", notes, "do not describe the same tree")
	})

	t.Run("an unstamped artifact is of unknown age", func(t *testing.T) {
		m, err := Load(writeGraph(t, map[string]any{
			"schema_version": SupportedSchema, "nodes": oneNode(), "edges": []any{}}))
		if err != nil {
			t.Fatal(err)
		}
		mentions(t, "the unstamped report", m.DisagreementsWith(4, 1), "no generation stamp")
	})

	// The one that was silent. A caller that could not read the index passes
	// zeroes, both comparisons are skipped, and the function returned an empty
	// slice — which every caller reads as "checked, and they agree". A check
	// that did not run must not answer the way a check that ran and passed does;
	// that is the rule this whole package is written around, and the staleness
	// comparison itself was the place it did not hold.
	t.Run("an index that could not be read is unverified, not agreed with", func(t *testing.T) {
		notes := stamped(4, 3).DisagreementsWith(0, 0)
		if len(notes) == 0 {
			t.Fatal("an artifact whose index could not be read was reported as agreeing with it; " +
				"the scope rung cannot tell that from a comparison that ran")
		}
		mentions(t, "the unverified report", notes, "unverified")
	})

	t.Run("a partly readable index still reports what it could not check", func(t *testing.T) {
		// The generation was readable and matches; the node count was not. The
		// half that ran must stay silent and the half that did not must speak.
		notes := stamped(4, 3).DisagreementsWith(4, 0)
		mentions(t, "the half-checked report", notes, "unverified")
		for _, n := range notes {
			if strings.Contains(n, "stands at") {
				t.Errorf("the generation comparison ran and agreed; it must not report: %v", notes)
			}
		}
	})
}

// TestTheProducersDiscardsAreReportedNotOnlyDecoded.
//
// The producer drops duplicate node ids and duplicate edges before it writes,
// and says how many in the same block that carries the orphan-endpoint count.
// Only the orphan count reached Degraded; the two discard counts were decoded
// into Provenance and one of them was never read at all. On this repository
// they are 103 nodes and 409 edges.
//
// A dropped duplicate node id is a symbol two nodes claimed, resolved by
// discarding one — and whichever one it was, its area is the area this map now
// uses for that identity. That is the same class of fact as an orphan endpoint:
// a coupling the map cannot see, reported so an empty answer is not read as an
// absent relation.
func TestTheProducersDiscardsAreReportedNotOnlyDecoded(t *testing.T) {
	m, err := Load(writeGraph(t, map[string]any{
		"schema_version": SupportedSchema, "nodes": oneNode(), "edges": []any{},
		"meta": map[string]any{"devmap_rust": map[string]any{
			"generation_id":              7,
			"duplicate_node_ids_dropped": 103,
			"duplicate_edges_dropped":    409,
		}},
	}))
	if err != nil {
		t.Fatal(err)
	}
	mentions(t, "the discard report", m.Degraded(), "103", "409")
}

// TestTheDistinctOrphanCountIsReportedWhenTheProducerGivesIt.
//
// `edge_endpoints_without_node` counts edges; `distinct_edge_endpoints_without_node`
// counts the identities those edges point at. On this repository they are 360
// and 167, and the ratio is the diagnostic: 360 edges lost to 360 different
// missing symbols is a wide, shallow gap, while 360 lost to one is a single
// symbol the extractor failed on. The remedies differ, and one number cannot
// distinguish them.
//
// It has to survive its own absence. Every producer built before the key
// existed emits nothing, Go decodes that as zero, and "360 edges naming 0
// distinct endpoints" is not a smaller report — it is an impossible one, which
// reads as a defect in the count rather than as a producer that predates it.
func TestTheDistinctOrphanCountIsReportedWhenTheProducerGivesIt(t *testing.T) {
	rust := func(extra map[string]any) *Map {
		t.Helper()
		block := map[string]any{"generation_id": 7, "edge_endpoints_without_node": 360}
		for k, v := range extra {
			block[k] = v
		}
		m, err := Load(writeGraph(t, map[string]any{
			"schema_version": SupportedSchema, "nodes": oneNode(), "edges": []any{},
			"meta": map[string]any{"devmap_rust": block},
		}))
		if err != nil {
			t.Fatal(err)
		}
		return m
	}

	t.Run("the producer emits it", func(t *testing.T) {
		m := rust(map[string]any{"distinct_edge_endpoints_without_node": 167})
		mentions(t, "the orphan report", m.Degraded(), "360", "167")
		if got := m.Provenance().DistinctOrphanEndpoints; got == nil || *got != 167 {
			t.Errorf("Provenance must carry the distinct count, got %v", got)
		}
	})

	t.Run("a producer that predates the key says nothing about it", func(t *testing.T) {
		m := rust(nil)
		if got := m.Provenance().DistinctOrphanEndpoints; got != nil {
			t.Errorf("an absent key must stay absent, got %v", *got)
		}
		mentions(t, "the orphan report", m.Degraded(), "360")
		if strings.Contains(strings.Join(m.Degraded(), " "), "0 distinct") {
			t.Errorf("an absent count was reported as zero: %v", m.Degraded())
		}
	})

	t.Run("a producer that emits a genuine zero is not an absent one", func(t *testing.T) {
		m := rust(map[string]any{
			"edge_endpoints_without_node":          0,
			"distinct_edge_endpoints_without_node": 0})
		if got := m.Provenance().DistinctOrphanEndpoints; got == nil || *got != 0 {
			t.Errorf("an explicit zero must be carried as a zero, got %v", got)
		}
	})
}

// TestAnAreaIsFoundWithoutWalkingEveryIndexedFile.
//
// AreaForPath falls back to the deepest indexed ancestor directory, which is
// the path taken for every file created during a turn — by definition the ones
// the scope rung exists to judge. It answered by scanning the whole path table
// once per ancestor level, so the cost of judging one write was the size of the
// repository times the depth of the path.
//
// This asserts the property rather than a duration: the answer must not change,
// and the work must not scale with the number of indexed files. The set it now
// consults is the one build already computed to count areas and then discarded.
func TestAnAreaIsFoundWithoutWalkingEveryIndexedFile(t *testing.T) {
	build := func(files int) *Map {
		t.Helper()
		ns := make([]map[string]any, 0, files)
		for i := 0; i < files; i++ {
			p := fmt.Sprintf("pkg/sub%d/f%d.go", i%8, i)
			ns = append(ns, map[string]any{
				"id": p, "kind": "file", "path": p, "area": fmt.Sprintf("pkg/sub%d", i%8)})
		}
		m, err := Load(writeGraph(t, map[string]any{
			"schema_version": SupportedSchema, "nodes": ns, "edges": []any{}}))
		if err != nil {
			t.Fatal(err)
		}
		return m
	}

	// The answer first: a new file under an indexed directory resolves to it,
	// and one under nothing indexed resolves to nothing.
	m := build(64)
	if area, ok := m.AreaForPath("pkg/sub3/written_this_turn.go"); !ok || area != "pkg/sub3" {
		t.Fatalf("a new file in an indexed area must resolve to it, got (%q,%v)", area, ok)
	}
	if area, ok := m.AreaForPath("elsewhere/entirely/x.go"); ok {
		t.Fatalf("a path under no indexed area must not resolve, got %q", area)
	}

	// And the cost. allocs are already zero on this path, so the measure has to
	// be work rather than memory: the miss is timed against a map eight times
	// larger holding the same areas. A scan of the path table grows with it; a
	// set lookup does not.
	small, large := build(256), build(2048)
	const probe = "elsewhere/entirely/deep/enough/x.go"
	ratio := timeLookups(large, probe) / timeLookups(small, probe)
	if ratio > 3 {
		t.Fatalf("resolving one path took %.1fx longer against 8x the files; the lookup is "+
			"walking the path table rather than consulting the set of areas", ratio)
	}
}

// timeLookups reports the cost of one AreaForPath miss, in nanoseconds.
//
// Each round runs for a fixed slice of time rather than a fixed number of
// calls, so the measurement costs the same whether the lookup is a set probe or
// a walk of the whole path table — a count tuned for the fast one takes seconds
// against the slow one, and a count tuned for the slow one measures noise
// against the fast one.
//
// The best of several rounds rather than one: the property under test is how the
// work scales with the size of the map, and a single sample on a shared machine
// measures the scheduler as much as the code. The minimum is the least
// contaminated estimate available without a quiet machine.
func timeLookups(m *Map, probe string) float64 {
	const budget = 20 * time.Millisecond
	// Batched so the clock is read once per batch rather than once per call,
	// which would otherwise be most of what the fast path measures.
	const batch = 64

	best := math.MaxFloat64
	for round := 0; round < 5; round++ {
		start := time.Now()
		calls := 0
		var elapsed time.Duration
		for {
			for i := 0; i < batch; i++ {
				m.AreaForPath(probe)
			}
			calls += batch
			if elapsed = time.Since(start); elapsed >= budget {
				break
			}
		}
		if ns := float64(elapsed.Nanoseconds()) / float64(calls); ns < best {
			best = ns
		}
	}
	return best
}

// TestAPartlyWrittenArtifactIsNeverReadAsAWholeOne.
//
// The producer writes this file atomically, so a reader should never see a
// prefix of it. "Should" is the operative word: the atomicity is another
// repository's property, the file sits in a shared state directory, and a
// consumer whose correctness rests on a guarantee it cannot check has no way to
// notice the day it stops holding.
//
// Every prefix of a real document is driven, because the interesting ones are
// not the obviously broken ones. A cut inside the node list leaves a document
// that is wrong in a way a lenient parser could paper over, and the answer that
// must never come back is a Map — a smaller graph read as a complete one is
// exactly the confident wrong answer this package exists to prevent.
func TestAPartlyWrittenArtifactIsNeverReadAsAWholeOne(t *testing.T) {
	whole, err := json.Marshal(map[string]any{
		"schema_version": SupportedSchema,
		"nodes": []map[string]any{
			{"id": "a/x.go", "kind": "file", "path": "a/x.go", "area": "a"},
			{"id": "b/y.go", "kind": "file", "path": "b/y.go", "area": "b"},
			{"id": "c/z.go", "kind": "file", "path": "c/z.go", "area": "c"},
		},
		"edges": []map[string]any{
			{"source": "a/x.go", "target": "b/y.go", "kind": "calls", "confidence": ConfidenceExtracted},
		},
		"meta": map[string]any{"devmap_rust": map[string]any{"generation_id": 4}},
	})
	if err != nil {
		t.Fatal(err)
	}

	dir := t.TempDir()
	p := filepath.Join(dir, "code_graph.json")
	accepted := 0
	for cut := 0; cut < len(whole); cut++ {
		if err := os.WriteFile(p, whole[:cut], 0o644); err != nil {
			t.Fatal(err)
		}
		m, err := Load(p)
		if err != nil {
			continue
		}
		// A prefix that parses is a prefix that was a whole document, which for
		// this fixture cannot happen — but if a future one could, it must at
		// least not have lost nodes silently.
		accepted++
		t.Errorf("the first %d of %d bytes loaded as a graph of %d file(s)",
			cut, len(whole), m.Stats().Files)
	}
	if accepted != 0 {
		t.Fatalf("%d truncated documents were read as whole ones", accepted)
	}

	// And the complete document still loads, so the check above is not passing
	// because everything is refused.
	if err := os.WriteFile(p, whole, 0o644); err != nil {
		t.Fatal(err)
	}
	m, err := Load(p)
	if err != nil {
		t.Fatalf("the whole document must load: %v", err)
	}
	if m.Stats().Files != 3 {
		t.Fatalf("the whole document holds 3 files, got %d", m.Stats().Files)
	}
}

// TestAnEmptyArtifactIsNotAnEmptyRepository. Zero bytes is what a creating
// process leaves behind between open and write, and what a failed write leaves
// behind for good. Neither is a repository with no code in it, and the
// difference decides whether the scope rung reports itself unavailable or
// answers "not in any area" to every question it is asked.
func TestAnEmptyArtifactIsNotAnEmptyRepository(t *testing.T) {
	for _, content := range []string{"", " \n\t ", "{}", `{"nodes":[]}`, `{"nodes":null}`} {
		p := filepath.Join(t.TempDir(), "code_graph.json")
		if err := os.WriteFile(p, []byte(content), 0o644); err != nil {
			t.Fatal(err)
		}
		if m, err := Load(p); err == nil {
			t.Errorf("%q loaded as a map of %d file(s); it holds no graph at all",
				content, m.Stats().Files)
		}
		// LoadIfPresent has to agree. Its nil-with-no-error is reserved for an
		// artifact that is not there, and a file that exists but holds nothing
		// is a different fact with a different remedy.
		m, err := LoadIfPresent(p)
		if err == nil {
			t.Errorf("LoadIfPresent(%q) reported no error and returned %v; an existing but "+
				"empty artifact is not an absent one", content, m)
		}
	}

	// The absent case, for the contrast the paragraph above turns on.
	m, err := LoadIfPresent(filepath.Join(t.TempDir(), "never-written.json"))
	if err != nil || m != nil {
		t.Fatalf("an absent artifact is (nil, nil), got (%v, %v)", m, err)
	}
}

// TestAReaderDuringARewriteSeesOneVersionOrAnError.
//
// Two readers and a writer over one path, which is the session-start shape: a
// background refresh rewriting the artifact while the gate loads it. The
// property is not that a reader wins the race — either version is a correct
// answer — but that no reader ever gets a map assembled from both, and that a
// reader which loses gets an error rather than a plausible half.
//
// The writer alternates between the atomic rename the producer uses and an
// in-place truncate-and-write, because the second is what a producer that
// dropped its temp file would do and the consumer's correctness must not rest
// on the producer's discipline.
func TestAReaderDuringARewriteSeesOneVersionOrAnError(t *testing.T) {
	graphOf := func(area string, files int) []byte {
		ns := make([]map[string]any, 0, files)
		for i := 0; i < files; i++ {
			p := fmt.Sprintf("%s/f%d.go", area, i)
			ns = append(ns, map[string]any{"id": p, "kind": "file", "path": p, "area": area})
		}
		raw, err := json.Marshal(map[string]any{
			"schema_version": SupportedSchema, "nodes": ns, "edges": []any{},
			"meta": map[string]any{"devmap_rust": map[string]any{"generation_id": files}},
		})
		if err != nil {
			t.Fatal(err)
		}
		return raw
	}
	// Two versions that cannot be confused for each other or for a blend: a
	// different area, a different size, a different generation.
	versions := [][]byte{graphOf("alpha", 40), graphOf("beta", 90)}

	dir := t.TempDir()
	p := filepath.Join(dir, "code_graph.json")
	if err := os.WriteFile(p, versions[0], 0o644); err != nil {
		t.Fatal(err)
	}

	stop := make(chan struct{})
	var writers, readers sync.WaitGroup
	writers.Add(1)
	go func() {
		defer writers.Done()
		for round := 0; ; round++ {
			select {
			case <-stop:
				return
			default:
			}
			want := versions[round%2]
			if round%2 == 0 {
				// The producer's own shape: write beside, then rename over.
				tmp := filepath.Join(dir, fmt.Sprintf("code_graph.json.tmp%d", round))
				if err := os.WriteFile(tmp, want, 0o644); err != nil {
					return
				}
				if err := os.Rename(tmp, p); err != nil {
					return
				}
				continue
			}
			// The shape a producer without a temp file would have: the reader
			// can see any prefix of this.
			f, err := os.OpenFile(p, os.O_WRONLY|os.O_CREATE|os.O_TRUNC, 0o644)
			if err != nil {
				return
			}
			for off := 0; off < len(want); off += 4096 {
				end := off + 4096
				if end > len(want) {
					end = len(want)
				}
				if _, err := f.Write(want[off:end]); err != nil {
					break
				}
			}
			f.Close()
		}
	}()

	var loads, refusals atomic.Int64
	for reader := 0; reader < 4; reader++ {
		readers.Add(1)
		go func() {
			defer readers.Done()
			for i := 0; i < 300; i++ {
				m, err := Load(p)
				if err != nil {
					refusals.Add(1)
					continue
				}
				loads.Add(1)
				s := m.Stats()
				prov := m.Provenance()
				// Whatever it read has to be exactly one of the two documents.
				// A blend would show as a file count that is neither, or as a
				// generation that does not match the size it came with.
				if s.Files != prov.GenerationID {
					t.Errorf("a map of %d file(s) carries generation %d; it was assembled "+
						"from more than one version of the artifact", s.Files, prov.GenerationID)
					return
				}
				if s.Files != 40 && s.Files != 90 {
					t.Errorf("a map of %d file(s) is neither version", s.Files)
					return
				}
				if s.Areas != 1 {
					t.Errorf("a map of one area's files holds %d areas", s.Areas)
					return
				}
			}
		}()
	}
	readers.Wait()
	close(stop)
	writers.Wait()

	if loads.Load() == 0 {
		t.Fatalf("no reader ever loaded the artifact (%d refusals); the race proved nothing",
			refusals.Load())
	}
	t.Logf("%d loads and %d refusals across the rewrite", loads.Load(), refusals.Load())
}

// TestAdversarialIdentifiersAreCarriedRatherThanCrashingOrTruncating.
//
// The ids, paths and areas in this file are strings the producer read out of
// source code, and source code is written by anyone. stress_test.go next door
// drives the shapes of the *document*; these are the shapes of the *strings*
// inside a well-formed one, which reach a map key, a path walk, and a report
// that gets printed.
//
// `want` is what the map must hold after the round trip, and it differs from
// the input in exactly one case, which is the point of stating it separately: a
// JSON string carrying bytes that are not UTF-8 decodes to the replacement
// character, so the path in the map is no longer the name of the file on disk.
// That is Go's decoder behaving as specified rather than a defect here — and it
// is a silent rename of an identifier the gate matches on, which is worth an
// assertion that pins it rather than a discovery later.
func TestAdversarialIdentifiersAreCarriedRatherThanCrashingOrTruncating(t *testing.T) {
	const huge = 10000
	hostile := []struct{ name, path, area, want string }{
		{"a ten-thousand character symbol path", strings.Repeat("s", huge) + ".go", "pkg", ""},
		{"a ten-thousand character area", "pkg/x.go", strings.Repeat("a", huge), ""},
		{"an embedded NUL", "pkg/x\x00y.go", "pkg\x00", ""},
		{"invalid UTF-8", "pkg/\xff\xfe.go", "pkg\xff", "pkg/��.go"},
		{"control characters", "pkg/\x01\x02\x1b[31m.go", "pkg\x07", ""},
		{"a path that is only separators", "///////", "//", ""},
		{"a path of parent references", strings.Repeat("../", 500) + "x.go", "..", ""},
		{"newlines inside the identifier", "pkg/x\ny\rz.go", "pkg\n", ""},
		{"a JSON fragment as a path", `pkg/{"id":"other"}.go`, `{"area":"x"}`, ""},
		{"a format verb as a path", "pkg/%s%d%v.go", "%!q(MISSING)", ""},
	}

	for _, h := range hostile {
		t.Run(h.name, func(t *testing.T) {
			want := h.want
			if want == "" {
				want = h.path
			}
			m, err := Load(writeGraph(t, map[string]any{
				"schema_version": SupportedSchema,
				"nodes": []map[string]any{
					{"id": "n1", "kind": "file", "path": h.path, "area": h.area},
					{"id": "n2", "kind": "file", "path": "other/y.go", "area": "other"},
				},
				"edges": []map[string]any{
					{"source": "n1", "target": "n2", "kind": "calls", "confidence": ConfidenceExtracted},
				},
			}))
			if err != nil {
				// Refusal is a legitimate answer; silence is not.
				return
			}
			checkInvariants(t, m)

			// The identifier must survive whole. A report that truncated it
			// would name a symbol that does not exist, and one that dropped it
			// would make the coupling invisible with nothing saying so.
			area, ok := m.AreaForPath(want)
			if !ok {
				t.Fatalf("the node's own path did not resolve to an area")
			}
			if !m.AreNeighbors(area, "other") {
				t.Errorf("the extracted coupling from %q to \"other\" was lost", area)
			}
			// And the reporting paths take it without panicking or looping.
			_ = m.Degraded()
			_ = m.DisagreementsWith(1, 1)
			_ = m.Neighbours(area)
			if got := m.Areas(); len(got) == 0 {
				t.Error("a map with two areas reported none")
			}
		})
	}
}

// TestBytesThatAreNotUTF8ReachTheMapAsTheDecoderLeavesThem.
//
// The case above goes through an encoder, which is not how the artifact is
// produced: the file is bytes some other process wrote, and it can hold a JSON
// string whose contents are not UTF-8 at all. This writes those bytes directly
// so nothing normalises them on the way in, and asserts the one property that
// matters — whatever the decoder makes of them, the map must not be built from
// a path it cannot reproduce, and it must not panic reading one.
func TestBytesThatAreNotUTF8ReachTheMapAsTheDecoderLeavesThem(t *testing.T) {
	// Written by hand: json.Marshal would replace these before they reached
	// the file, which is the thing this test exists to bypass.
	raw := []byte(`{"schema_version":2,"nodes":[` +
		`{"id":"n1","kind":"file","path":"pkg/` + "\xff\xfe" + `.go","area":"pkg` + "\xff" + `"},` +
		`{"id":"n2","kind":"file","path":"other/y.go","area":"other"}],` +
		`"edges":[{"source":"n1","target":"n2","kind":"calls","confidence":"extracted"}]}`)

	p := filepath.Join(t.TempDir(), "code_graph.json")
	if err := os.WriteFile(p, raw, 0o644); err != nil {
		t.Fatal(err)
	}
	m, err := Load(p)
	if err != nil {
		// A decoder that refuses the bytes outright is a fine answer.
		t.Logf("the artifact was refused: %v", err)
		return
	}
	checkInvariants(t, m)

	// Whatever path the map holds, it must answer for that path — a map keyed
	// on something none of its own accessors can name would degrade every
	// query about the file with nothing saying why.
	found := false
	for _, area := range m.Areas() {
		if area.Files == 0 {
			continue
		}
		for _, n := range m.Neighbours(area.Name) {
			if n != "" {
				found = true
			}
		}
	}
	if !found {
		t.Error("the coupling between the two areas was lost entirely")
	}
	if area, ok := m.AreaForPath("other/y.go"); !ok || area != "other" {
		t.Errorf("the well-formed node beside the hostile one stopped resolving: (%q,%v)", area, ok)
	}
}

// TestADeeplyNestedDocumentIsAVerdictNotACrash. The artifact is handed to a
// decoder that recurses into the values it is decoding, and a file of nothing
// but opening brackets is the cheapest way to turn that into a stack
// exhaustion — which on this path takes down a session at its first frame, over
// a file some other tool left in a shared directory.
func TestADeeplyNestedDocumentIsAVerdictNotACrash(t *testing.T) {
	for _, where := range []struct{ name, doc string }{
		{"under an unknown key", `{"unknown":` + strings.Repeat("[", 200000) + strings.Repeat("]", 200000) + `}`},
		{"under the node list", `{"nodes":` + strings.Repeat("[", 200000) + strings.Repeat("]", 200000) + `}`},
		{"unclosed", `{"nodes":` + strings.Repeat("[", 200000)},
		{"inside a node's own fields", `{"nodes":[{"id":"a","path":` +
			strings.Repeat("[", 100000) + strings.Repeat("]", 100000) + `}]}`},
	} {
		t.Run(where.name, func(t *testing.T) {
			p := filepath.Join(t.TempDir(), "code_graph.json")
			if err := os.WriteFile(p, []byte(where.doc), 0o644); err != nil {
				t.Fatal(err)
			}
			m, err := Load(p)
			if err != nil {
				return
			}
			checkInvariants(t, m)
		})
	}
}

// TestOneMapAnswersManyReadersAtOnce.
//
// A Map is built once and then only read, and the gate consults it per write —
// so more than one decision can be in flight over the same map. Nothing in the
// type says so, and "only read" is a property of the current implementation
// rather than a guarantee: a memo, a lazily-filled cache, a stat counter added
// later would each be a write, and each would be invisible until it produced a
// wrong answer under load rather than a crash.
//
// Run under -race this is the assertion that no accessor mutates what it reads.
// Provenance is included deliberately: it hands out a pointer, and a pointer
// into the map's own state would let one reader's caller change what the next
// reader sees.
func TestOneMapAnswersManyReadersAtOnce(t *testing.T) {
	var nodes []map[string]any
	for i := 0; i < 400; i++ {
		area := fmt.Sprintf("pkg/sub%d", i%20)
		p := fmt.Sprintf("%s/f%d.go", area, i)
		nodes = append(nodes, map[string]any{"id": p, "kind": "file", "path": p, "area": area})
	}
	var edges []map[string]any
	for i := 0; i < 400; i++ {
		edges = append(edges, map[string]any{
			"source": fmt.Sprintf("pkg/sub%d/f%d.go", i%20, i),
			"target": fmt.Sprintf("pkg/sub%d/f%d.go", (i+1)%20, (i+37)%400),
			"kind":   "calls", "confidence": ConfidenceExtracted})
	}
	m, err := Load(writeGraph(t, map[string]any{
		"schema_version": SupportedSchema, "nodes": nodes, "edges": edges,
		"meta": map[string]any{"devmap_rust": map[string]any{
			"generation_id": 4, "edge_endpoints_without_node": 7,
			"distinct_edge_endpoints_without_node": 3}},
	}))
	if err != nil {
		t.Fatal(err)
	}

	// One reader's answers, taken before any other starts, are what every other
	// reader has to agree with.
	wantStats := m.Stats()
	wantDegraded := strings.Join(m.Degraded(), "|")
	wantAreas := len(m.Areas())

	var readers sync.WaitGroup
	for r := 0; r < 8; r++ {
		readers.Add(1)
		go func(r int) {
			defer readers.Done()
			for i := 0; i < 500; i++ {
				if got := m.Stats(); got != wantStats {
					t.Errorf("reader %d saw %+v, want %+v", r, got, wantStats)
					return
				}
				if got := strings.Join(m.Degraded(), "|"); got != wantDegraded {
					t.Errorf("reader %d saw a different report: %q", r, got)
					return
				}
				if got := len(m.Areas()); got != wantAreas {
					t.Errorf("reader %d counted %d areas, want %d", r, got, wantAreas)
					return
				}
				area, _ := m.AreaForPath(fmt.Sprintf("pkg/sub%d/new%d.go", i%20, i))
				_ = m.AreNeighbors(area, "pkg/sub0")
				_ = m.Neighbours(area)
				_ = m.NeighborsArePermissive()
				_ = m.DisagreementsWith(4, 400)

				// The pointer Provenance hands out must be this reader's own.
				// Writing through it and reading back an unchanged map is the
				// property; a shared pointer would make this reader's scribble
				// the next one's provenance.
				prov := m.Provenance()
				if prov.DistinctOrphanEndpoints == nil {
					t.Errorf("reader %d lost the distinct count", r)
					return
				}
				*prov.DistinctOrphanEndpoints = r
				if again := m.Provenance(); again.DistinctOrphanEndpoints == nil ||
					*again.DistinctOrphanEndpoints != 3 {
					t.Errorf("reader %d changed the map's own provenance through the "+
						"pointer it was handed", r)
					return
				}
			}
		}(r)
	}
	readers.Wait()
}
