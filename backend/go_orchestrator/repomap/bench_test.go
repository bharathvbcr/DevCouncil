package repomap

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"testing"
)

// What this package costs is worth measuring because of when it is paid. Load
// runs at session start, before the first frame, on a file that grows with the
// repository; AreaForPath runs once per write the gate judges. Neither had a
// number attached, so neither had a way to notice a change that made it worse.
//
// The fixture is sized from this repository's own artifact rather than from a
// round number, so the figures mean something against a real tree: 14,330
// nodes over 1,328 files in 221 areas, 74,061 edges of which 68% are `calls`
// and 65% carry the resolved confidence. Generating it takes longer than the
// benchmark, so it is built once and shared.

// realisticShape is this repository's code graph, in the proportions the
// producer actually writes.
var realisticShape = struct {
	files, symbolsPerFile, areas int
	edges                        int
	resolvedPercent              int
}{files: 1328, symbolsPerFile: 10, areas: 221, edges: 74061, resolvedPercent: 65}

// realisticGraph builds a document of that shape. It is deterministic, so two
// benchmark runs measure the same work.
func realisticGraph() []byte {
	s := realisticShape
	nodes := make([]map[string]any, 0, s.files*(1+s.symbolsPerFile))
	ids := make([]string, 0, cap(nodes))
	for f := 0; f < s.files; f++ {
		area := fmt.Sprintf("pkg/group%d/sub%d", f%s.areas/16, f%s.areas)
		path := fmt.Sprintf("%s/file%d.go", area, f)
		nodes = append(nodes, map[string]any{
			"id": path, "kind": "file", "path": path, "area": area,
			"community": fmt.Sprintf("community-%d", f%40)})
		ids = append(ids, path)
		for sym := 0; sym < s.symbolsPerFile; sym++ {
			id := fmt.Sprintf("%s::Symbol%d", path, sym)
			nodes = append(nodes, map[string]any{
				"id": id, "kind": "function", "path": path, "area": area,
				"community": fmt.Sprintf("community-%d", f%40)})
			ids = append(ids, id)
		}
	}

	// The kind mix the producer writes, and the confidence split that decides
	// how much of it reaches adjacency at all.
	kinds := []string{"calls", "calls", "calls", "calls", "calls", "calls", "calls",
		"contains", "contains", "references", "imports"}
	edges := make([]map[string]any, 0, s.edges)
	for e := 0; e < s.edges; e++ {
		confidence := "ambiguous"
		if e%100 < s.resolvedPercent {
			confidence = ConfidenceExtracted
		}
		edges = append(edges, map[string]any{
			"source": ids[(e*7)%len(ids)], "target": ids[(e*13+3)%len(ids)],
			"kind": kinds[e%len(kinds)], "confidence": confidence, "reason": ""})
	}

	raw, err := json.Marshal(map[string]any{
		"schema_version": SupportedSchema, "nodes": nodes, "edges": edges,
		"meta": map[string]any{"map_engine": "devmap-rust", "devmap_rust": map[string]any{
			"generation_id": 40, "analysis_status": "ok",
			"edge_endpoints_without_node": 360, "duplicate_edges_dropped": 409,
			"duplicate_node_ids_dropped": 103}},
	})
	if err != nil {
		panic(err)
	}
	return raw
}

// graphFixture writes the document once for the whole run and returns its path
// and bytes. Regenerating it per benchmark would cost more than the benchmarks.
var graphFixture = struct {
	once bool
	path string
	raw  []byte
}{}

func fixture(b *testing.B) (string, []byte) {
	b.Helper()
	if !graphFixture.once {
		graphFixture.raw = realisticGraph()
		graphFixture.path = filepath.Join(b.TempDir(), "code_graph.json")
		if err := os.WriteFile(graphFixture.path, graphFixture.raw, 0o644); err != nil {
			b.Fatal(err)
		}
		graphFixture.once = true
	}
	// A later b.TempDir() is a different directory, so the file is rewritten if
	// the first one has gone; the bytes are what is expensive, not the write.
	if _, err := os.Stat(graphFixture.path); err != nil {
		graphFixture.path = filepath.Join(b.TempDir(), "code_graph.json")
		if err := os.WriteFile(graphFixture.path, graphFixture.raw, 0o644); err != nil {
			b.Fatal(err)
		}
	}
	return graphFixture.path, graphFixture.raw
}

// BenchmarkLoadRealisticGraph is the session-start cost: read the artifact and
// derive the map. It is the figure MaxGraphBytes is set against.
func BenchmarkLoadRealisticGraph(b *testing.B) {
	path, raw := fixture(b)
	b.SetBytes(int64(len(raw)))
	b.ReportAllocs()
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		if _, err := Load(path); err != nil {
			b.Fatal(err)
		}
	}
}

// BenchmarkDecodeRealisticGraph is the same work without the derivation, so the
// two halves can be told apart rather than guessed at.
func BenchmarkDecodeRealisticGraph(b *testing.B) {
	_, raw := fixture(b)
	b.SetBytes(int64(len(raw)))
	b.ReportAllocs()
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		var g graph
		if err := json.Unmarshal(raw, &g); err != nil {
			b.Fatal(err)
		}
	}
}

// BenchmarkBuildRealisticGraph is the derivation alone: one pass over 14,330
// nodes and 74,061 edges, which is the only part of Load this package owns.
func BenchmarkBuildRealisticGraph(b *testing.B) {
	_, raw := fixture(b)
	var g graph
	if err := json.Unmarshal(raw, &g); err != nil {
		b.Fatal(err)
	}
	b.ReportAllocs()
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		build(g)
	}
}

// BenchmarkAreaForPath is the per-write cost, in the three shapes a path
// arrives in: already indexed, new inside an indexed area, and outside
// everything. The third was the one that scaled with the size of the map.
func BenchmarkAreaForPath(b *testing.B) {
	path, _ := fixture(b)
	m, err := Load(path)
	if err != nil {
		b.Fatal(err)
	}
	indexed := "pkg/group0/sub0/file0.go"
	if _, ok := m.AreaForPath(indexed); !ok {
		b.Fatalf("the fixture does not hold %s; the benchmark would measure the miss path twice", indexed)
	}
	for _, probe := range []struct{ name, path string }{
		{"indexed", indexed},
		{"new file in an indexed area", "pkg/group0/sub0/written_this_turn.go"},
		{"outside every indexed area", "vendor/third/party/deep/thing.go"},
	} {
		b.Run(probe.name, func(b *testing.B) {
			b.ReportAllocs()
			for i := 0; i < b.N; i++ {
				m.AreaForPath(probe.path)
			}
		})
	}
}

// BenchmarkAreas is the reporting path: it walks every indexed file to count
// them per area, and the harness calls it to tell a model what the repository
// is made of.
func BenchmarkAreas(b *testing.B) {
	path, _ := fixture(b)
	m, err := Load(path)
	if err != nil {
		b.Fatal(err)
	}
	b.ReportAllocs()
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		m.Areas()
	}
}

// BenchmarkAreNeighbors is the question the scope rung actually asks, once the
// two areas are known.
func BenchmarkAreNeighbors(b *testing.B) {
	path, _ := fixture(b)
	m, err := Load(path)
	if err != nil {
		b.Fatal(err)
	}
	areas := m.Areas()
	if len(areas) < 2 {
		b.Fatalf("the fixture holds %d areas", len(areas))
	}
	b.ReportAllocs()
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		m.AreNeighbors(areas[i%len(areas)].Name, areas[(i+1)%len(areas)].Name)
	}
}
