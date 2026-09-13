package repomap

import (
	"fmt"
	"strings"
	"testing"
)

func TestInternedTableRejectsRepeatedStructuralKeys(t *testing.T) {
	for name, table := range map[string]string{
		"fields after rows":  `"fields":[["id","s"]],"rows":[[0]],"fields":[["kind","j"]]`,
		"fields before rows": `"fields":[["id","s"]],"fields":[["kind","s"]],"rows":[[0]]`,
		"rows repeated":      `"fields":[["id","s"]],"rows":[[0]],"rows":[[0]]`,
	} {
		t.Run(name, func(t *testing.T) {
			raw := fmt.Sprintf(`{"schema_version":%d,"encoding":%q,"strings":["node"],"interned_tables":["nodes"],"nodes":{%s},"edges":[]}`, SupportedSchema, CompactEncoding, table)
			_, err := Load(writeRawGraph(t, "code_graph.compact.json", []byte(raw)))
			if err == nil || !strings.Contains(err.Error(), "repeated") {
				t.Fatalf("repeated structural key must be refused: %v", err)
			}
		})
	}
}

func TestCompactGraphRejectsRepeatedTopLevelStructure(t *testing.T) {
	for _, key := range []string{"strings", "interned_tables", "nodes", "edges", "schema_version", "encoding"} {
		t.Run(key, func(t *testing.T) {
			raw := fmt.Sprintf(`{"schema_version":%d,"encoding":%q,"strings":[],"interned_tables":["nodes"],"nodes":{"fields":[["id","s"]],"rows":[]},"edges":[],%q:null}`, SupportedSchema, CompactEncoding, key)
			if _, err := Load(writeRawGraph(t, "code_graph.compact.json", []byte(raw))); err == nil || !strings.Contains(err.Error(), "repeated") {
				t.Fatalf("repeated %s must be refused: %v", key, err)
			}
		})
	}
}
