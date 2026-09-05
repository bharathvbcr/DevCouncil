package repomap

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"sort"
	"strings"
	"testing"
)

// The interned encoding is a second wire form of the same model, written by a
// producer built from another workspace. What has to be true of it is not that
// this package can parse it — that is easy to arrange and easy to be wrong
// about — but that a map built from it is the *same map* as one built from the
// verbose artifact. Anything less and the gate's answer would depend on which
// file the harness happened to read.
//
// So the assertions here are equality of whole `Map` values rather than of
// selected fields: areas, neighbours, stats, provenance and the vocabulary
// record all at once. A field this package starts deriving tomorrow is covered
// without anyone remembering to add it.

// internTables is the encoder's shape, written here so a test can produce the
// interned form without the producer being present.
//
// It is deliberately not the decoder's inverse written by the same hand and
// left at that: `TestTheLiveCompactGraphBuildsTheSameMap` in `dc/devmap` drives
// the real kernel and is what settles that this shape is the one that arrives.
// This helper exists so the ordinary tests are hermetic and the adversarial
// ones have something well-formed to damage.
func internTables(t testing.TB, plain []byte) []byte {
	t.Helper()
	var document map[string]json.RawMessage
	if err := json.Unmarshal(plain, &document); err != nil {
		t.Fatalf("the verbose fixture is not JSON: %v", err)
	}

	// Sorted, like the producer's: its string table is a function of the set of
	// strings rather than of the order they were found in.
	pool := map[string]bool{}
	type table struct {
		names []string
		kinds []string
		rows  []map[string]json.RawMessage
	}
	tables := map[string]*table{}
	for _, name := range []string{"nodes", "edges"} {
		body, ok := document[name]
		if !ok {
			continue
		}
		var rows []map[string]json.RawMessage
		if err := json.Unmarshal(body, &rows); err != nil {
			t.Fatalf("%s is not an array of objects: %v", name, err)
		}
		if len(rows) == 0 {
			tables[name] = &table{}
			continue
		}
		var names []string
		for key := range rows[0] {
			names = append(names, key)
		}
		sort.Strings(names)
		kinds := make([]string, len(names))
		for position, column := range names {
			kinds[position] = columnInterned
			for _, row := range rows {
				cell, ok := row[column]
				if !ok || len(cell) == 0 || cell[0] != '"' {
					kinds[position] = columnRaw
					break
				}
			}
		}
		for _, row := range rows {
			for position, column := range names {
				if kinds[position] != columnInterned {
					continue
				}
				var text string
				if err := json.Unmarshal(row[column], &text); err != nil {
					t.Fatal(err)
				}
				pool[text] = true
			}
		}
		tables[name] = &table{names: names, kinds: kinds, rows: rows}
	}

	strs := make([]string, 0, len(pool))
	for text := range pool {
		strs = append(strs, text)
	}
	sort.Strings(strs)
	index := make(map[string]int, len(strs))
	for position, text := range strs {
		index[text] = position
	}

	out := map[string]json.RawMessage{}
	for key, value := range document {
		out[key] = value
	}
	var internedNames []string
	for name, tbl := range tables {
		internedNames = append(internedNames, name)
		fields := make([][2]string, len(tbl.names))
		for position, column := range tbl.names {
			fields[position] = [2]string{column, tbl.kinds[position]}
		}
		rows := make([][]json.RawMessage, 0, len(tbl.rows))
		for _, row := range tbl.rows {
			cells := make([]json.RawMessage, len(tbl.names))
			for position, column := range tbl.names {
				if tbl.kinds[position] == columnInterned {
					var text string
					if err := json.Unmarshal(row[column], &text); err != nil {
						t.Fatal(err)
					}
					cells[position] = json.RawMessage(fmt.Sprintf("%d", index[text]))
					continue
				}
				cells[position] = row[column]
			}
			rows = append(rows, cells)
		}
		encoded, err := json.Marshal(map[string]any{"fields": fields, "rows": rows})
		if err != nil {
			t.Fatal(err)
		}
		out[name] = encoded
	}
	sort.Strings(internedNames)

	for key, value := range map[string]any{
		"encoding":        CompactEncoding,
		"strings":         strs,
		"interned_tables": internedNames,
		"verbatim_tables": []string{},
	} {
		encoded, err := json.Marshal(value)
		if err != nil {
			t.Fatal(err)
		}
		out[key] = encoded
	}

	raw, err := json.Marshal(out)
	if err != nil {
		t.Fatal(err)
	}
	return raw
}

// compactFixture is a graph with everything the map derives from: two coupled
// areas, one area coupled to nothing, an unresolved edge, an edge naming an
// endpoint that is not a node, and a full provenance block.
func compactFixture(t testing.TB) []byte {
	t.Helper()
	schema := SupportedSchema
	raw, err := json.Marshal(map[string]any{
		"schema_version": schema,
		"nodes": []map[string]any{
			{"id": "pkg/gate/gate.go", "kind": "file", "path": "pkg/gate/gate.go",
				"area": "pkg/gate", "community": "community-1", "line": 0, "exported": false,
				"extras": map[string]any{}},
			{"id": "pkg/gate/gate.go::Decide", "kind": "function", "path": "pkg/gate/gate.go",
				"area": "pkg/gate", "community": "community-1", "line": 12, "exported": true,
				"extras": map[string]any{"qualname": "Decide"}},
			{"id": "pkg/policy/policy.go", "kind": "file", "path": "pkg/policy/policy.go",
				"area": "pkg/policy", "community": "community-2", "line": 0, "exported": false,
				"extras": map[string]any{}},
			{"id": "pkg/policy/policy.go::Allow", "kind": "function", "path": "pkg/policy/policy.go",
				"area": "pkg/policy", "community": "community-2", "line": 30, "exported": true,
				"extras": map[string]any{}},
			{"id": "docs/readme.md", "kind": "file", "path": "docs/readme.md",
				"area": "docs", "community": "community-3", "line": 0, "exported": false,
				"extras": map[string]any{}},
		},
		"edges": []map[string]any{
			{"source": "pkg/gate/gate.go::Decide", "target": "pkg/policy/policy.go::Allow",
				"kind": "calls", "confidence": ConfidenceExtracted, "reason": "",
				"extras": map[string]any{}},
			{"source": "pkg/policy/policy.go::Allow", "target": "docs/readme.md",
				"kind": "calls", "confidence": "ambiguous", "reason": "",
				"extras": map[string]any{}},
			{"source": "pkg/gate/gate.go::Decide", "target": "pkg/nowhere/x.go::Gone",
				"kind": "calls", "confidence": ConfidenceExtracted, "reason": "",
				"extras": map[string]any{}},
		},
		"meta": map[string]any{
			"map_engine": "devmap-rust",
			"devmap_rust": map[string]any{
				"generation_id": 7, "analysis_status": "ok",
				"edge_endpoints_without_node":          1,
				"distinct_edge_endpoints_without_node": 1,
				"duplicate_edges_dropped":              2,
				"duplicate_node_ids_dropped":           3,
			},
		},
	})
	if err != nil {
		t.Fatal(err)
	}
	return raw
}

func writeRawGraph(t testing.TB, name string, body []byte) string {
	t.Helper()
	p := filepath.Join(t.TempDir(), name)
	if err := os.WriteFile(p, body, 0o644); err != nil {
		t.Fatal(err)
	}
	return p
}

// TestTheInternedEncodingBuildsTheSameMap is the equivalence assertion. Two
// encodings of one model must not be two answers.
func TestTheInternedEncodingBuildsTheSameMap(t *testing.T) {
	plain := compactFixture(t)
	compact := internTables(t, plain)

	if len(compact) >= len(plain) {
		t.Errorf("the interned form is %d bytes against %d verbose; the fixture is too small "+
			"to be exercising the encoding", len(compact), len(plain))
	}

	fromPlain, err := Load(writeRawGraph(t, "code_graph.json", plain))
	if err != nil {
		t.Fatalf("the verbose artifact did not load: %v", err)
	}
	fromCompact, err := Load(writeRawGraph(t, "code_graph.compact.json", compact))
	if err != nil {
		t.Fatalf("the interned artifact did not load: %v", err)
	}

	// Named comparisons first, so a failure says which half moved before the
	// whole-value one says only that something did.
	if !reflect.DeepEqual(fromPlain.Stats(), fromCompact.Stats()) {
		t.Errorf("stats differ:\n verbose  %+v\n interned %+v", fromPlain.Stats(), fromCompact.Stats())
	}
	if !reflect.DeepEqual(fromPlain.Provenance(), fromCompact.Provenance()) {
		t.Errorf("provenance differs:\n verbose  %+v\n interned %+v",
			fromPlain.Provenance(), fromCompact.Provenance())
	}
	if !reflect.DeepEqual(fromPlain.Areas(), fromCompact.Areas()) {
		t.Errorf("areas differ:\n verbose  %+v\n interned %+v", fromPlain.Areas(), fromCompact.Areas())
	}
	for _, area := range fromPlain.Areas() {
		if !reflect.DeepEqual(fromPlain.Neighbours(area.Name), fromCompact.Neighbours(area.Name)) {
			t.Errorf("neighbours of %s differ: %v against %v", area.Name,
				fromPlain.Neighbours(area.Name), fromCompact.Neighbours(area.Name))
		}
	}
	if !reflect.DeepEqual(fromPlain.Degraded(), fromCompact.Degraded()) {
		t.Errorf("degradations differ:\n verbose  %v\n interned %v",
			fromPlain.Degraded(), fromCompact.Degraded())
	}
	// And everything, including what no test names yet.
	if !reflect.DeepEqual(fromPlain, fromCompact) {
		t.Errorf("the two encodings built different maps:\n verbose  %+v\n interned %+v",
			fromPlain, fromCompact)
	}

	// The fixture has to be one where these answers are non-trivial, or the
	// comparison above would hold for two empty maps.
	if !fromCompact.AreNeighbors("pkg/gate", "pkg/policy") {
		t.Error("the interned map lost the one resolved coupling in the fixture")
	}
	if fromCompact.AreNeighbors("pkg/gate", "docs") {
		t.Error("the interned map admitted an unresolved edge as adjacency")
	}
	if got, ok := fromCompact.AreaForPath("pkg/policy/policy.go"); !ok || got != "pkg/policy" {
		t.Errorf("AreaForPath from the interned map = %q, %v", got, ok)
	}
}

// mutate rewrites one top-level key of an interned document, so the adversarial
// cases below start from something well-formed and damage exactly one thing.
func mutate(t testing.TB, compact []byte, edits map[string]any) []byte {
	t.Helper()
	var document map[string]json.RawMessage
	if err := json.Unmarshal(compact, &document); err != nil {
		t.Fatal(err)
	}
	for key, value := range edits {
		if value == nil {
			delete(document, key)
			continue
		}
		encoded, err := json.Marshal(value)
		if err != nil {
			t.Fatal(err)
		}
		document[key] = encoded
	}
	raw, err := json.Marshal(document)
	if err != nil {
		t.Fatal(err)
	}
	return raw
}

// TestAnInternedGraphIsRefusedRatherThanShortened is the Class A pass over the
// decoder. Every case here is a document that could be read as a *smaller*
// graph — fewer nodes, fewer edges, a missing area — and a smaller graph is
// what makes the gate answer "not a neighbour" with confidence. So each one has
// to be a refusal, and the refusal has to name what it found.
func TestAnInternedGraphIsRefusedRatherThanShortened(t *testing.T) {
	base := internTables(t, compactFixture(t))

	for _, probe := range []struct {
		name string
		body []byte
		want string
	}{
		{
			// A layout this build does not know must not be read as the one it
			// does. The producer stamps the identity for exactly this.
			name: "a layout identity this build has never seen",
			body: mutate(t, base, map[string]any{"encoding": "devmap-compact-v2"}),
			want: "devmap-compact-v2",
		},
		{
			// The interned tables are indices into a table that is no longer
			// there. Reading them as anything would be inventing strings.
			name: "the string table is gone",
			body: mutate(t, base, map[string]any{"strings": nil}),
			want: "string table",
		},
		{
			name: "an index past the end of the string table",
			body: func() []byte {
				var document map[string]json.RawMessage
				if err := json.Unmarshal(base, &document); err != nil {
					t.Fatal(err)
				}
				var table struct {
					Fields [][2]string         `json:"fields"`
					Rows   [][]json.RawMessage `json:"rows"`
				}
				if err := json.Unmarshal(document["nodes"], &table); err != nil {
					t.Fatal(err)
				}
				table.Rows[0][0] = json.RawMessage("999999")
				encoded, err := json.Marshal(table)
				if err != nil {
					t.Fatal(err)
				}
				document["nodes"] = encoded
				raw, err := json.Marshal(document)
				if err != nil {
					t.Fatal(err)
				}
				return raw
			}(),
			want: "999999",
		},
		{
			name: "a column declaring a storage kind that is neither",
			body: func() []byte {
				var document map[string]json.RawMessage
				if err := json.Unmarshal(base, &document); err != nil {
					t.Fatal(err)
				}
				var table map[string]json.RawMessage
				if err := json.Unmarshal(document["nodes"], &table); err != nil {
					t.Fatal(err)
				}
				var fields [][2]string
				if err := json.Unmarshal(table["fields"], &fields); err != nil {
					t.Fatal(err)
				}
				fields[0][1] = "z"
				encoded, err := json.Marshal(fields)
				if err != nil {
					t.Fatal(err)
				}
				table["fields"] = encoded
				body, err := json.Marshal(table)
				if err != nil {
					t.Fatal(err)
				}
				document["nodes"] = body
				raw, err := json.Marshal(document)
				if err != nil {
					t.Fatal(err)
				}
				return raw
			}(),
			want: "storage",
		},
		{
			name: "a row with fewer cells than the field list declares",
			body: func() []byte {
				var document map[string]json.RawMessage
				if err := json.Unmarshal(base, &document); err != nil {
					t.Fatal(err)
				}
				var table struct {
					Fields [][2]string         `json:"fields"`
					Rows   [][]json.RawMessage `json:"rows"`
				}
				if err := json.Unmarshal(document["nodes"], &table); err != nil {
					t.Fatal(err)
				}
				table.Rows[1] = table.Rows[1][:len(table.Rows[1])-1]
				encoded, err := json.Marshal(table)
				if err != nil {
					t.Fatal(err)
				}
				document["nodes"] = encoded
				raw, err := json.Marshal(document)
				if err != nil {
					t.Fatal(err)
				}
				return raw
			}(),
			want: "cells",
		},
		{
			// The producer never writes this, and a decoder that let it through
			// would resolve the same identity to two different strings.
			name: "a field list naming one column twice",
			body: func() []byte {
				var document map[string]json.RawMessage
				if err := json.Unmarshal(base, &document); err != nil {
					t.Fatal(err)
				}
				var table map[string]json.RawMessage
				if err := json.Unmarshal(document["nodes"], &table); err != nil {
					t.Fatal(err)
				}
				var fields [][2]string
				if err := json.Unmarshal(table["fields"], &fields); err != nil {
					t.Fatal(err)
				}
				fields[1][0] = fields[0][0]
				encoded, err := json.Marshal(fields)
				if err != nil {
					t.Fatal(err)
				}
				table["fields"] = encoded
				body, err := json.Marshal(table)
				if err != nil {
					t.Fatal(err)
				}
				document["nodes"] = body
				raw, err := json.Marshal(document)
				if err != nil {
					t.Fatal(err)
				}
				return raw
			}(),
			want: "twice",
		},
		{
			name: "an interned cell holding a string instead of an index",
			body: func() []byte {
				var document map[string]json.RawMessage
				if err := json.Unmarshal(base, &document); err != nil {
					t.Fatal(err)
				}
				var table struct {
					Fields [][2]string         `json:"fields"`
					Rows   [][]json.RawMessage `json:"rows"`
				}
				if err := json.Unmarshal(document["nodes"], &table); err != nil {
					t.Fatal(err)
				}
				table.Rows[0][0] = json.RawMessage(`"pkg/gate"`)
				encoded, err := json.Marshal(table)
				if err != nil {
					t.Fatal(err)
				}
				document["nodes"] = encoded
				raw, err := json.Marshal(document)
				if err != nil {
					t.Fatal(err)
				}
				return raw
			}(),
			want: "index",
		},
		{
			// Truncation is the shape a partial write takes on disk, and the
			// only wrong answer is a graph that loads and is short.
			name: "the file stops partway through",
			body: base[:len(base)*3/4],
			want: "unexpected EOF",
		},
	} {
		t.Run(probe.name, func(t *testing.T) {
			_, err := Load(writeRawGraph(t, "code_graph.compact.json", probe.body))
			if err == nil {
				t.Fatal("this document must be refused, not read as a smaller graph")
			}
			if !strings.Contains(err.Error(), probe.want) {
				t.Errorf("the refusal must name %q; got: %v", probe.want, err)
			}
		})
	}
}

// TestAnInternedGraphWithNoRowsIsNotBuiltAtAll.
//
// An interned document whose tables are empty decodes perfectly and describes
// nothing. That is the same condition as a verbose artifact with `nodes: []`,
// which this package already refuses, and it has to be refused on both wires or
// the harness's answer would depend on which one it read.
func TestAnInternedGraphWithNoRowsIsNotBuiltAtAll(t *testing.T) {
	empty, err := json.Marshal(map[string]any{
		"schema_version":  SupportedSchema,
		"encoding":        CompactEncoding,
		"strings":         []string{},
		"interned_tables": []string{"edges", "nodes"},
		"verbatim_tables": []string{},
		"nodes":           map[string]any{"fields": [][2]string{{"id", "s"}}, "rows": [][]int{}},
		"edges":           map[string]any{"fields": [][2]string{{"source", "s"}}, "rows": [][]int{}},
	})
	if err != nil {
		t.Fatal(err)
	}
	_, loadErr := Load(writeRawGraph(t, "code_graph.compact.json", empty))
	if loadErr == nil {
		t.Fatal("an interned graph with no rows must be refused the way an empty verbose one is")
	}
	if !strings.Contains(loadErr.Error(), "holds no nodes") {
		t.Errorf("the refusal must be the same one the verbose wire gives; got: %v", loadErr)
	}
}

// TestAnInternedGraphIsHeldToTheSameBound.
//
// MaxGraphBytes exists because loading costs several times the artifact in
// transient memory, and the interned wire does not change that: the string
// table and the row indices are both proportional to the file. A bound that
// applied to one encoding and not the other would be no bound at all, since the
// producer chooses which file the consumer is pointed at.
func TestAnInternedGraphIsHeldToTheSameBound(t *testing.T) {
	body := internTables(t, compactFixture(t))
	p := writeRawGraph(t, "code_graph.compact.json", body)
	if _, err := loadBounded(p, int64(len(body))); err != nil {
		t.Fatalf("a document exactly at the bound must load: %v", err)
	}
	_, err := loadBounded(p, int64(len(body))-1)
	if err == nil {
		t.Fatal("a document past the bound must be refused before it is decoded")
	}
	for _, want := range []string{"larger than", "bound"} {
		if !strings.Contains(err.Error(), want) {
			t.Errorf("the refusal must name the bound (%q); got: %v", want, err)
		}
	}
}

// TestDuplicateIdentitiesSurviveBothWiresIdentically.
//
// The producer drops duplicate node ids before it writes and says how many, but
// nothing stops a hand-edited or older artifact from carrying them, and the two
// encodings must resolve them the same way — the map answers with whichever
// node the decode kept, and "whichever" has to be the same one on both wires or
// the same repository would be judged differently depending on the file read.
func TestDuplicateIdentitiesSurviveBothWiresIdentically(t *testing.T) {
	plain, err := json.Marshal(map[string]any{
		"schema_version": SupportedSchema,
		"nodes": []map[string]any{
			{"id": "dup", "kind": "function", "path": "pkg/a/x.go", "area": "pkg/a",
				"community": "c1", "line": 1, "extras": map[string]any{}},
			{"id": "dup", "kind": "function", "path": "pkg/b/y.go", "area": "pkg/b",
				"community": "c2", "line": 2, "extras": map[string]any{}},
			{"id": "pkg/c/z.go::Z", "kind": "function", "path": "pkg/c/z.go", "area": "pkg/c",
				"community": "c3", "line": 3, "extras": map[string]any{}},
		},
		"edges": []map[string]any{
			{"source": "dup", "target": "pkg/c/z.go::Z", "kind": "calls",
				"confidence": ConfidenceExtracted, "reason": "", "extras": map[string]any{}},
			{"source": "dup", "target": "pkg/c/z.go::Z", "kind": "calls",
				"confidence": ConfidenceExtracted, "reason": "", "extras": map[string]any{}},
		},
		"meta": map[string]any{"devmap_rust": map[string]any{"generation_id": 1}},
	})
	if err != nil {
		t.Fatal(err)
	}
	compact := internTables(t, plain)

	fromPlain, err := Load(writeRawGraph(t, "code_graph.json", plain))
	if err != nil {
		t.Fatal(err)
	}
	fromCompact, err := Load(writeRawGraph(t, "code_graph.compact.json", compact))
	if err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(fromPlain, fromCompact) {
		t.Errorf("duplicate identities resolved differently:\n verbose  %+v\n interned %+v",
			fromPlain, fromCompact)
	}
}

// TestAVerbatimTableOnTheInternedWireIsStillRead.
//
// The encoder carries a table through unchanged when its rows are not uniformly
// shaped, and names it in `verbatim_tables` rather than dropping it. A decoder
// that assumed every table in a compact document was interned would read that
// as a missing table — the graph would load, be short, and say nothing.
func TestAVerbatimTableOnTheInternedWireIsStillRead(t *testing.T) {
	plain := compactFixture(t)
	compact := internTables(t, plain)

	// Put `edges` back in its verbose shape, which is what the encoder does for
	// a table it declines to intern.
	var verbose map[string]json.RawMessage
	if err := json.Unmarshal(plain, &verbose); err != nil {
		t.Fatal(err)
	}
	mixed := mutate(t, compact, map[string]any{
		"interned_tables": []string{"nodes"},
		"verbatim_tables": []string{"edges"},
	})
	var document map[string]json.RawMessage
	if err := json.Unmarshal(mixed, &document); err != nil {
		t.Fatal(err)
	}
	document["edges"] = verbose["edges"]
	body, err := json.Marshal(document)
	if err != nil {
		t.Fatal(err)
	}

	fromPlain, err := Load(writeRawGraph(t, "code_graph.json", plain))
	if err != nil {
		t.Fatal(err)
	}
	fromMixed, err := Load(writeRawGraph(t, "code_graph.compact.json", body))
	if err != nil {
		t.Fatalf("a compact document with one verbatim table did not load: %v", err)
	}
	if !reflect.DeepEqual(fromPlain, fromMixed) {
		t.Errorf("a verbatim table decoded to a different map:\n verbose %+v\n mixed   %+v",
			fromPlain, fromMixed)
	}
}

// TestTheVerboseWireIsUnchangedByTheInternedOne.
//
// The decoder now dispatches on the shape of each table, and the verbose
// artifact is the one every existing consumer reads. Its failures have to stay
// failures: a document that is not an object, one whose tables are the wrong
// type, and trailing bytes after the graph.
func TestTheVerboseWireIsUnchangedByTheInternedOne(t *testing.T) {
	for _, probe := range []struct{ name, body string }{
		{"an array where the graph should be", `[{"id":"a"}]`},
		{"nodes that are neither rows nor a table", `{"nodes":7,"edges":[]}`},
		{"a second document after the first", `{"nodes":[],"edges":[]}{"nodes":[]}`},
		{"nothing at all", ``},
		{"a truncated object", `{"nodes":[{"id":"a"`},
	} {
		t.Run(probe.name, func(t *testing.T) {
			if _, err := Load(writeRawGraph(t, "code_graph.json", []byte(probe.body))); err == nil {
				t.Fatal("this is not a code graph and must be refused")
			}
		})
	}
}

// TestTheWidestAreaIsTheSameAreaOnEveryLoad.
//
// Stats.WidestArea names the area the neighbour rule is loosest around, and an
// operator reads it to decide whether to rely on the rule at all. It was picked
// by walking `adjacent`, which is a Go map, and keeping the first area with a
// degree strictly greater than the best so far — so on the ties that a small or
// regular repository produces constantly, the name reported was whichever key
// the runtime happened to hand over first. The same file loaded twice in one
// process named two different areas.
//
// That is a report changing under a reader with nothing about the repository
// having changed, and it is what the equivalence assertion above tripped over:
// two encodings of one graph cannot be compared while one field of the answer
// is decided by map iteration order.
func TestTheWidestAreaIsTheSameAreaOnEveryLoad(t *testing.T) {
	// Two areas with one neighbour each: a tie, which is the case the ordering
	// decided rather than the data.
	raw, err := json.Marshal(map[string]any{
		"schema_version": SupportedSchema,
		"nodes": []map[string]any{
			{"id": "pkg/a/x.go", "kind": "file", "path": "pkg/a/x.go", "area": "pkg/a"},
			{"id": "pkg/b/y.go", "kind": "file", "path": "pkg/b/y.go", "area": "pkg/b"},
		},
		"edges": []map[string]any{
			{"source": "pkg/a/x.go", "target": "pkg/b/y.go", "kind": "calls",
				"confidence": ConfidenceExtracted},
		},
		"meta": map[string]any{"devmap_rust": map[string]any{"generation_id": 1}},
	})
	if err != nil {
		t.Fatal(err)
	}
	p := writeRawGraph(t, "code_graph.json", raw)

	first, err := Load(p)
	if err != nil {
		t.Fatal(err)
	}
	want := first.Stats().WidestArea
	for attempt := 0; attempt < 64; attempt++ {
		again, err := Load(p)
		if err != nil {
			t.Fatal(err)
		}
		if got := again.Stats().WidestArea; got != want {
			t.Fatalf("load %d named %q as the widest area and the first named %q, from the "+
				"same file; the report depends on map iteration order rather than on the graph",
				attempt+2, got, want)
		}
	}
}
