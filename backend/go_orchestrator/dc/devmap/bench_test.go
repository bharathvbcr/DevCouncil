package devmap

import (
	"bytes"
	"fmt"
	"strings"
	"testing"
)

// Two things in this package do real work in-process rather than waiting on a
// subprocess, and both scale with what the producer wrote rather than with what
// was asked of it. Neither had a number attached.
//
// The marker scan is the expensive one and it is the one that runs first: adopt
// opens both artifacts at the top of every manifest, and the code graph is 20 MB
// on this repository. Its cost is the trade artifact.go documents — it retains
// one token instead of the document, and pays for that in churn — so the figure
// to watch is allocations per byte scanned, which is what a change to that trade
// would move.

// artifactOf builds a document of the given size with the engine marker last,
// which is where the real one carries it: a scan cannot answer without crossing
// the whole file.
func artifactOf(nodes int) []byte {
	var b bytes.Buffer
	b.WriteString(`{"schema_version":2,"nodes":[`)
	for i := 0; i < nodes; i++ {
		if i > 0 {
			b.WriteByte(',')
		}
		fmt.Fprintf(&b, `{"id":"pkg/group%d/file%d.go::Symbol%d","kind":"function",`+
			`"path":"pkg/group%d/file%d.go","area":"pkg/group%d","language":"go",`+
			`"line":%d,"end_line":%d,"exported":true,"extras":{}}`,
			i%64, i/10, i%10, i%64, i/10, i%64, i, i+40)
	}
	b.WriteString(`],"edges":[],"meta":{"map_engine":"devmap-rust"}}`)
	return b.Bytes()
}

// BenchmarkScanMarker is the adoption check, at this repository's node count.
func BenchmarkScanMarker(b *testing.B) {
	doc := artifactOf(14330)
	b.SetBytes(int64(len(doc)))
	b.ReportAllocs()
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		engine, ok := scanMarker(bytes.NewReader(doc), []string{"meta", "map_engine"})
		if !ok || engine != consumerMapEngine {
			b.Fatalf("engine=%q wellFormed=%v", engine, ok)
		}
	}
}

// BenchmarkReadNotices is the stderr classifier, at the producer's own limit of
// twenty named refusals plus a header.
func BenchmarkReadNotices(b *testing.B) {
	var lines []string
	lines = append(lines, "discovery refused 25 file(s)")
	for i := 0; i < 20; i++ {
		lines = append(lines, fmt.Sprintf("  pkg/group%d/generated_%d.rs: Unreadable { reason: \"not utf-8\" }", i%8, i))
	}
	lines = append(lines, "… and 5 more")
	stream := said{text: []byte(strings.Join(lines, "\n"))}

	b.SetBytes(int64(len(stream.text)))
	b.ReportAllocs()
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		if n := readNotices(stream); n.Refused != 25 {
			b.Fatalf("refused=%d", n.Refused)
		}
	}
}

// BenchmarkCappedWrite is the bound every subprocess stream passes through, on
// the ordinary path where nothing is dropped.
func BenchmarkCappedWrite(b *testing.B) {
	chunk := make([]byte, 32<<10)
	b.SetBytes(int64(len(chunk)))
	b.ReportAllocs()
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		c := &capped{limit: len(chunk) * 8, hard: true}
		for w := 0; w < 8; w++ {
			if _, err := c.Write(chunk); err != nil {
				b.Fatal(err)
			}
		}
		if c.truncated {
			b.Fatal("a stream of exactly the bound is not truncated")
		}
	}
}
