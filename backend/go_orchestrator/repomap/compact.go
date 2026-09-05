package repomap

// Reading the interned encoding.
//
// The producer writes the same model twice on request: `code_graph.json`, which
// every existing consumer reads, and beside it an interned form in which each
// distinct string is written once and referred to by index. On a 4,499-file
// corpus the two are 85,001,360 B and 17,104,715 B — 78,429 distinct strings
// written 1,084,960 times, and the verbose file spends most of itself repeating
// them.
//
// The encoder's own account of the layout is `DIVERGENCES.md` row X39 and
// `encode_compact` in `devmap-query/src/code_graph.rs`. What matters here:
//
//   - `encoding` names the layout. A decoder that read an unknown layout as
//     this one would answer from a shape it does not understand, so a value
//     this build does not know is refused rather than attempted.
//   - `strings` is the shared table. Every interned cell is an index into it.
//   - `interned_tables` / `verbatim_tables` say which tables were interned. A
//     table whose rows are not uniformly shaped is carried through in its
//     verbose form, so one document can hold both shapes at once, and a decoder
//     that assumed otherwise would read a real table as a missing one.
//   - Each interned table is `{"fields": [[name, kind], …], "rows": [[…], …]}`,
//     where kind is `s` for an index into the string table and `j` for the
//     value itself.
//
// # Why this decodes rather than reconstructs
//
// The Rust `decode_compact` rebuilds the whole verbose document, because its
// contract is the round trip. This package reads nine string columns out of
// seventeen, so it fills `[]node` and `[]edge` directly and steps over the
// rest: `extras`, `line`, `end_line`, `exported`, `language`, `name` and
// `reason` never reach a `Map`, and the interned wire is the only one where not
// reading them is cheaper than reading them.
//
// # Why the rows are scanned rather than decoded
//
// The first version of this read every cell through `json.Decoder.Token`, and
// measured on the corpus above it was faster than the verbose path while
// allocating *more*: 12.6 million allocations against 1.6, because a `Token`
// call boxes each value and reading indices exactly requires `UseNumber`, which
// turns every one of them into a heap string. The rows are the one place where
// the shape is fully known — an array of arrays of small integers — so they are
// scanned in place, and `encoding/json` is left to do what it is good at, which
// is validating and framing the document around them. Nothing outside a row is
// hand-parsed.
//
// # Why there is one decoder and not two
//
// `Load` dispatches on the shape of each table rather than on a flag, and both
// wires converge on the same `graph` value before `build` sees anything. Two
// decode paths that each produced a `Map` would be two implementations of the
// map, and the failure would not be a crash — it would be a gate that answered
// "not a neighbour" on one wire and "neighbour" on the other, with nothing to
// notice which file it had read.

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
)

// CompactEncoding is the layout identity this build reads.
//
// It is checked rather than assumed for the same reason SupportedSchema is: the
// producer is built from another workspace, and a layout that changed shape
// under a name this build still recognised would be decoded into a confident,
// wrong graph.
const CompactEncoding = "devmap-compact-v1"

// The two storage kinds a column can declare: an index into the string table,
// or the value written out. An unrecognised third is refused — see readFields.
const (
	columnInterned = "s"
	columnRaw      = "j"
)

// ErrUnknownCompactLayout reports an interned artifact this build cannot read.
//
// It is a distinct error rather than a parse failure because the remedies are
// not the same: a parse failure means the file is damaged, and this means the
// producer moved ahead of the consumer and the harness needs rebuilding. They
// send an operator to different places.
var ErrUnknownCompactLayout = errors.New("interned code graph layout this build does not read")

// The columns this package reads, in the order the row constructors expect.
var (
	nodeColumns = []string{"id", "kind", "path", "area", "community"}
	edgeColumns = []string{"source", "target", "kind", "confidence"}
)

func nodeFromCells(cells []string) node {
	return node{ID: cells[0], Kind: cells[1], Path: cells[2], Area: cells[3], Community: cells[4]}
}

func edgeFromCells(cells []string) edge {
	return edge{Source: cells[0], Target: cells[1], Kind: cells[2], Confidence: cells[3]}
}

// decodeGraph reads either encoding into one graph.
//
// The walk is over the top-level object's keys, because the two facts an
// interned table needs — the string table and the layout identity — are not
// where its rows are. In the producer's output `strings` sorts after `nodes`
// and `encoding` sorts after `edges`, so a decoder that resolved indices as it
// met them would be reading a table it had not seen. Indices are therefore
// collected as indices and resolved once the document is closed, which is also
// what makes it possible to refuse an artifact that never declares its layout
// before a single string has been invented for it.
func decodeGraph(raw []byte) (graph, error) {
	decoder := json.NewDecoder(bytes.NewReader(raw))

	opening, err := decoder.Token()
	if err != nil {
		return graph{}, err
	}
	if delimiter, ok := opening.(json.Delim); !ok || delimiter != '{' {
		return graph{}, fmt.Errorf("a code graph is a JSON object and this document opens with %v", opening)
	}

	var (
		g              graph
		encodingSeen   bool
		pool           []string
		poolSeen       bool
		pendingNodes   *pendingTable
		pendingEdges   *pendingTable
		internedTables []string
	)

	for decoder.More() {
		token, err := decoder.Token()
		if err != nil {
			return graph{}, err
		}
		key, ok := token.(string)
		if !ok {
			return graph{}, fmt.Errorf("a code graph's keys are strings and this document has %v", token)
		}
		switch key {
		case "encoding":
			var encoding string
			if err := decoder.Decode(&encoding); err != nil {
				return graph{}, fmt.Errorf("`encoding` is not a string: %w", err)
			}
			if encoding != CompactEncoding {
				return graph{}, fmt.Errorf("%w: the artifact declares %q and this build reads %q; "+
					"it was refused rather than read as a layout it is not",
					ErrUnknownCompactLayout, encoding, CompactEncoding)
			}
			encodingSeen = true
		case "strings":
			if err := decoder.Decode(&pool); err != nil {
				return graph{}, fmt.Errorf("the string table is not an array of strings: %w", err)
			}
			poolSeen = true
		case "interned_tables":
			if err := decoder.Decode(&internedTables); err != nil {
				return graph{}, fmt.Errorf("`interned_tables` is not a list of names: %w", err)
			}
		case "schema_version":
			if err := decoder.Decode(&g.SchemaVersion); err != nil {
				return graph{}, fmt.Errorf("`schema_version` is not a number: %w", err)
			}
		case "meta":
			if err := decoder.Decode(&g.Meta); err != nil {
				return graph{}, fmt.Errorf("`meta` is not the provenance block: %w", err)
			}
		case "nodes":
			pendingNodes, err = readTable(decoder, key, nodeColumns, &g.Nodes)
			if err != nil {
				return graph{}, err
			}
		case "edges":
			pendingEdges, err = readTable(decoder, key, edgeColumns, &g.Edges)
			if err != nil {
				return graph{}, err
			}
		default:
			// Everything else in the artifact — `dead_code`, `entry_roots`, the
			// fingerprints — belongs to another consumer. Handing it to a value
			// that keeps none of it means the decoder validates and frames it
			// without building anything, which is the cheapest correct way of
			// not reading something.
			if err := decoder.Decode(&discarded{}); err != nil {
				return graph{}, err
			}
		}
	}
	if _, err := decoder.Token(); err != nil {
		return graph{}, err
	}
	// A second document after the first is not a longer graph, and a decoder
	// that stopped at the closing brace would read the first of two artifacts
	// concatenated by a failed write as though it were the whole thing.
	if _, err := decoder.Token(); !errors.Is(err, io.EOF) {
		if err != nil {
			return graph{}, err
		}
		return graph{}, errors.New("the document continues past the end of the code graph")
	}

	if (pendingNodes != nil || pendingEdges != nil) && !encodingSeen {
		return graph{}, fmt.Errorf("%w: %s in an interned layout that names no `encoding`, so "+
			"nothing establishes which layout its indices belong to",
			ErrUnknownCompactLayout, internedNames(pendingNodes != nil, pendingEdges != nil))
	}
	if encodingSeen {
		if !poolSeen {
			return graph{}, errors.New("the artifact declares the interned encoding and carries " +
				"no string table, so every interned cell in it names a string that is not there")
		}
		// The producer names the tables it interned, and this build reads the
		// shape each table arrived in. They are two accounts of one fact, and a
		// document where they disagree is one whose `verbatim_tables` cannot be
		// believed either — and that is the field telling a consumer a
		// verbose-looking table is the encoder reporting it could not intern,
		// rather than a table that was silently dropped.
		declared := make(map[string]bool, len(internedTables))
		for _, name := range internedTables {
			declared[name] = true
		}
		for _, table := range []struct {
			name  string
			found bool
		}{{"nodes", pendingNodes != nil}, {"edges", pendingEdges != nil}} {
			if declared[table.name] == table.found {
				continue
			}
			return graph{}, fmt.Errorf("`interned_tables` %s %s and the table itself %s interned, "+
				"so the artifact's account of what was interned is not the account its own rows give",
				namesIt(declared[table.name]), table.name, wasIt(table.found))
		}
	}
	if err := resolve(pendingNodes, pool, nodeFromCells, &g.Nodes); err != nil {
		return graph{}, err
	}
	if err := resolve(pendingEdges, pool, edgeFromCells, &g.Edges); err != nil {
		return graph{}, err
	}
	return g, nil
}

// namesIt and wasIt render the two halves of that disagreement, so the refusal
// reads as a sentence rather than as two booleans.
func namesIt(present bool) string {
	if present {
		return "names"
	}
	return "does not name"
}

func wasIt(present bool) string {
	if present {
		return "is"
	}
	return "is not"
}

// internedNames renders the tables found in the interned shape, so the refusal
// above names what it saw rather than only what it wanted.
func internedNames(nodes, edges bool) string {
	switch {
	case nodes && edges:
		return "`nodes` and `edges` are"
	case nodes:
		return "`nodes` is"
	default:
		return "`edges` is"
	}
}

// discarded reads any JSON and keeps none of it, so a key this package does not
// use costs the decoder's validation and nothing else.
type discarded struct{}

func (discarded) UnmarshalJSON([]byte) error { return nil }

// compactColumn is one column of an interned table that this package reads.
type compactColumn struct {
	// cell is its position in a row.
	cell int
	// slot is its position in the row constructor's argument.
	slot int
	// interned says whether the cell holds an index or the value.
	interned bool
}

// pendingTable is an interned table read but not yet resolved.
//
// The cells are kept as int32 indices rather than as strings because that is
// the whole point of the encoding: the 271,240 edge rows of the corpus above
// hold 1,084,960 interned cells, and holding them as strings before the table
// arrived would rebuild the verbose artifact in memory to avoid reading it from
// disk. Resolved, they are headers into the one shared table, so an identity
// written 1,084,960 times is stored once.
//
// It is not generic over the row type: nothing here depends on it, and the row
// constructor arrives at resolve.
type pendingTable struct {
	name    string
	columns []compactColumn
	// width is how many cells the field list declares a row has; a row of any
	// other length is a refusal rather than a row with a missing column.
	width int
	rows  int
	// index holds one entry per (row, interned column) in column order.
	index []int32
	// literal holds one entry per (row, raw column). The producer interns every
	// column whose every row holds a string, so this is normally empty; it is
	// here because a column that holds a non-string in even one row is written
	// out in full for every row, and this package still has to read it.
	literal []string
	// internedColumns and rawColumns are the strides of the two stores.
	internedColumns int
	rawColumns      int
	// slots is how many fields the row constructor takes.
	slots int
}

// readTable reads either shape of one table.
//
// A verbose table is decoded straight into out, because nothing about it needs
// the rest of the document. An interned one is returned for resolution once the
// string table has been seen.
func readTable[T any](decoder *json.Decoder, name string, want []string, out *[]T) (*pendingTable, error) {
	token, err := decoder.Token()
	if err != nil {
		return nil, err
	}
	switch shape := token.(type) {
	case nil:
		// `"nodes": null` decodes to no rows on the verbose wire and must here.
		return nil, nil
	case json.Delim:
		switch shape {
		case '[':
			// One row variable for the whole table, not one per row. `&row`
			// escapes into the decoder's interface argument, so declaring it
			// inside the loop put every row of the verbose artifact on the heap
			// twice — 90,811 extra allocations and 5.9 MB on the fixture, paid
			// by the encoding every existing consumer reads. It is reset each
			// time because a decode leaves a field the row omitted holding
			// whatever the previous row put there.
			var row, empty T
			for decoder.More() {
				row = empty
				if err := decoder.Decode(&row); err != nil {
					return nil, fmt.Errorf("a row of %s: %w", name, err)
				}
				*out = append(*out, row)
			}
			if _, err := decoder.Token(); err != nil {
				return nil, err
			}
			return nil, nil
		case '{':
			return readInternedTable(decoder, name, want)
		}
	}
	return nil, fmt.Errorf("%s is neither an array of rows nor an interned table; it holds %v",
		name, token)
}

// readInternedTable reads `{"fields": …, "rows": …}` for one table.
//
// The field list must precede the rows, and it does in every document the
// producer writes: its object keys are sorted and `fields` sorts before `rows`.
// Requiring it is what lets the rows be read in one pass; a document that
// inverts them is refused saying so, rather than read by a second pass nobody
// would ever measure.
func readInternedTable(decoder *json.Decoder, name string, want []string) (*pendingTable, error) {
	table := &pendingTable{name: name, slots: len(want)}
	wanted := make(map[string]int, len(want))
	for slot, column := range want {
		wanted[column] = slot
	}

	var fieldsSeen, rowsSeen bool
	for decoder.More() {
		token, err := decoder.Token()
		if err != nil {
			return nil, err
		}
		key, ok := token.(string)
		if !ok {
			return nil, fmt.Errorf("the interned table %s has a non-string key %v", name, token)
		}
		switch key {
		case "fields":
			if err := table.readFields(decoder, wanted); err != nil {
				return nil, err
			}
			fieldsSeen = true
		case "rows":
			if !fieldsSeen {
				return nil, fmt.Errorf("the interned table %s puts its rows before its field "+
					"list, so there is nothing that says what its cells are", name)
			}
			if err := decoder.Decode(&rowBlock{table: table}); err != nil {
				return nil, err
			}
			rowsSeen = true
		default:
			if err := decoder.Decode(&discarded{}); err != nil {
				return nil, err
			}
		}
	}
	if _, err := decoder.Token(); err != nil {
		return nil, err
	}
	if !fieldsSeen {
		return nil, fmt.Errorf("the interned table %s carries no field list, so its rows are "+
			"positions with no names", name)
	}
	if !rowsSeen {
		return nil, fmt.Errorf("the interned table %s carries no rows; an absent table and an "+
			"empty one are not the same claim", name)
	}
	return table, nil
}

// readFields parses the column layout and refuses anything it cannot account
// for. A column whose storage kind is neither of the two is the case that
// matters: reading it as one of them would put an index where a value belongs.
func (t *pendingTable) readFields(decoder *json.Decoder, wanted map[string]int) error {
	var fields [][]string
	if err := decoder.Decode(&fields); err != nil {
		return fmt.Errorf("the field list of %s is not a list of [name, storage] pairs: %w", t.name, err)
	}
	seen := make(map[string]bool, len(fields))
	for position, field := range fields {
		if len(field) != 2 {
			return fmt.Errorf("field %d of %s is not a [name, storage] pair", position, t.name)
		}
		name, kind := field[0], field[1]
		if seen[name] {
			return fmt.Errorf("the field list of %s names %q twice, so a cell of it would resolve "+
				"to two different values", t.name, name)
		}
		seen[name] = true
		if kind != columnInterned && kind != columnRaw {
			return fmt.Errorf("field %q of %s declares storage %q, which is neither %q nor %q; "+
				"this build cannot tell whether its cells are indices or values",
				name, t.name, kind, columnInterned, columnRaw)
		}
		slot, ok := wanted[name]
		if !ok {
			continue
		}
		t.columns = append(t.columns, compactColumn{
			cell: position, slot: slot, interned: kind == columnInterned,
		})
		if kind == columnInterned {
			t.internedColumns++
		} else {
			t.rawColumns++
		}
	}
	t.width = len(fields)
	return nil
}

// rowBlock hands the rows array to the scanner below.
//
// It is an Unmarshaler rather than a Token loop so that `encoding/json` still
// validates the array and finds its bounds — the bytes handed over are already
// known to be a well-formed JSON array — while the per-cell work, which is
// where the allocations were, happens over those bytes in place.
type rowBlock struct{ table *pendingTable }

func (b *rowBlock) UnmarshalJSON(data []byte) error { return b.table.scanRows(data) }

// scanRows walks the rows array.
func (t *pendingTable) scanRows(data []byte) error {
	at := skipSpace(data, 0)
	if at >= len(data) || data[at] != '[' {
		return fmt.Errorf("the rows of %s are not an array", t.name)
	}
	at++
	for {
		at = skipSpace(data, at)
		if at >= len(data) {
			return fmt.Errorf("the rows of %s end without closing", t.name)
		}
		if data[at] == ']' {
			return nil
		}
		next, err := t.scanRow(data, at)
		if err != nil {
			return err
		}
		t.rows++
		at = skipSpace(data, next)
		if at >= len(data) {
			return fmt.Errorf("the rows of %s end without closing", t.name)
		}
		switch data[at] {
		case ',':
			at++
		case ']':
			return nil
		default:
			return fmt.Errorf("row %d of %s is followed by %q rather than by another row",
				t.rows-1, t.name, data[at])
		}
	}
}

// scanRow reads one row and returns the offset just past it.
//
// A row whose cell count is not the width the field list declared is refused
// rather than padded: cells after the declared width have no name, and a cell
// missing before it silently shifts every column after the gap. That is the
// shape of failure this encoding makes possible and the verbose one does not,
// where a missing key is simply a missing key.
func (t *pendingTable) scanRow(data []byte, at int) (int, error) {
	if data[at] != '[' {
		return 0, fmt.Errorf("row %d of %s is not an array of cells", t.rows, t.name)
	}
	at++
	cells, next := 0, 0
	for {
		at = skipSpace(data, at)
		if at >= len(data) {
			return 0, fmt.Errorf("row %d of %s ends without closing", t.rows, t.name)
		}
		if data[at] == ']' {
			at++
			break
		}
		start, end, err := valueRange(data, at)
		if err != nil {
			return 0, fmt.Errorf("cell %d of row %d of %s: %w", cells, t.rows, t.name, err)
		}
		if next < len(t.columns) && t.columns[next].cell == cells {
			if t.columns[next].interned {
				slot, err := parseIndex(data[start:end])
				if err != nil {
					return 0, fmt.Errorf("cell %d of row %d of %s: %w", cells, t.rows, t.name, err)
				}
				t.index = append(t.index, slot)
			} else {
				text, err := parseRawString(data[start:end])
				if err != nil {
					return 0, fmt.Errorf("cell %d of row %d of %s: %w", cells, t.rows, t.name, err)
				}
				t.literal = append(t.literal, text)
			}
			next++
		}
		cells++
		at = skipSpace(data, end)
		if at >= len(data) {
			return 0, fmt.Errorf("row %d of %s ends without closing", t.rows, t.name)
		}
		switch data[at] {
		case ',':
			at++
		case ']':
			at++
			if cells != t.width {
				return 0, t.wrongWidth(cells)
			}
			return at, nil
		default:
			return 0, fmt.Errorf("cell %d of row %d of %s is followed by %q",
				cells-1, t.rows, t.name, data[at])
		}
	}
	if cells != t.width {
		return 0, t.wrongWidth(cells)
	}
	return at, nil
}

func (t *pendingTable) wrongWidth(cells int) error {
	return fmt.Errorf("row %d of %s has %d cells for %d fields", t.rows, t.name, cells, t.width)
}

// valueRange returns the bounds of the JSON value starting at `at`.
//
// The bytes have already been validated as JSON by the decoder that framed the
// rows array, so this walks structure rather than checking it. What it has to
// get right is where a value ends: a string holding a bracket, and an object
// nested inside a cell — `extras` is one, and it is why a cell cannot simply be
// scanned to the next comma.
func valueRange(data []byte, at int) (int, int, error) {
	switch data[at] {
	case '"':
		for cursor := at + 1; cursor < len(data); {
			switch data[cursor] {
			case '\\':
				cursor += 2
			case '"':
				return at, cursor + 1, nil
			default:
				cursor++
			}
		}
		return 0, 0, errors.New("a quoted value is not closed")
	case '{', '[':
		depth := 0
		for cursor := at; cursor < len(data); {
			switch data[cursor] {
			case '"':
				_, end, err := valueRange(data, cursor)
				if err != nil {
					return 0, 0, err
				}
				cursor = end
			case '{', '[':
				depth++
				cursor++
			case '}', ']':
				depth--
				cursor++
				if depth == 0 {
					return at, cursor, nil
				}
			default:
				cursor++
			}
		}
		return 0, 0, errors.New("a nested value is not closed")
	default:
		for cursor := at; cursor < len(data); cursor++ {
			switch data[cursor] {
			case ',', ']', '}', ' ', '\t', '\n', '\r':
				return at, cursor, nil
			}
		}
		return 0, 0, errors.New("a value is not terminated")
	}
}

// parseIndex reads one interned cell.
//
// The encoder writes a plain non-negative integer and nothing else, so nothing
// else is accepted. A sign, a fraction or an exponent would each have to be
// given a meaning this layout does not define, and giving one to a cell that
// names a symbol identity is how a graph comes to describe the wrong symbol.
func parseIndex(cell []byte) (int32, error) {
	if len(cell) == 0 {
		return 0, errors.New("an interned column holds nothing where an index into the string table belongs")
	}
	var value int64
	for _, digit := range cell {
		if digit < '0' || digit > '9' {
			return 0, fmt.Errorf("%q is not an index into the string table", cell)
		}
		value = value*10 + int64(digit-'0')
		// Bounded before it can overflow, because an index that wrapped would
		// land back inside the table and resolve to a real string from another
		// row — a wrong answer rather than a refusal.
		if value > maxStringIndex {
			return 0, fmt.Errorf("%q is not an index into the string table", cell)
		}
	}
	return int32(value), nil
}

// maxStringIndex is the largest index this build will read. No string table
// inside MaxGraphBytes can reach it — one entry costs at least three bytes on
// the wire — so the ceiling refuses only nonsense.
const maxStringIndex = 1<<31 - 1

// parseRawString reads a cell of a column the encoder wrote out rather than
// interning it, in the two forms the verbose wire would have accepted for a
// string field: the string itself, and null for the absent one.
func parseRawString(cell []byte) (string, error) {
	if bytes.Equal(cell, []byte("null")) {
		return "", nil
	}
	if len(cell) == 0 || cell[0] != '"' {
		return "", fmt.Errorf("a column this build reads as text holds %s", cell)
	}
	var text string
	if err := json.Unmarshal(cell, &text); err != nil {
		return "", err
	}
	return text, nil
}

func skipSpace(data []byte, at int) int {
	for at < len(data) {
		switch data[at] {
		case ' ', '\t', '\n', '\r':
			at++
		default:
			return at
		}
	}
	return at
}

// resolve turns the collected indices into rows.
//
// A nil table is one that was never interned, which is not an error: its
// verbose rows were appended as they were read.
func resolve[T any](t *pendingTable, pool []string, row func([]string) T, out *[]T) error {
	if t == nil {
		return nil
	}
	*out = make([]T, 0, t.rows)
	cells := make([]string, t.slots)
	for index := 0; index < t.rows; index++ {
		for slot := range cells {
			cells[slot] = ""
		}
		interned, raw := 0, 0
		for _, column := range t.columns {
			if column.interned {
				slot := t.index[index*t.internedColumns+interned]
				if int(slot) >= len(pool) {
					return fmt.Errorf("row %d of %s names string %d of the %d in the table, so "+
						"the artifact and its string table are not from the same write",
						index, t.name, slot, len(pool))
				}
				cells[column.slot] = pool[slot]
				interned++
				continue
			}
			cells[column.slot] = t.literal[index*t.rawColumns+raw]
			raw++
		}
		*out = append(*out, row(cells))
	}
	return nil
}
