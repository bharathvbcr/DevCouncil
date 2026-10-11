package verify

import "testing"

// The store refuses a gap whose evidence is not a JSON array, and a nil
// slice marshals to null: a gap built without evidence must still be storable.
func TestToStoreGapsWritesEmptyEvidenceAsAnArray(t *testing.T) {
	rows := toStoreGaps([]Gap{{ID: "G1", Severity: "high", GapType: "x"}})
	if got := string(rows[0].EvidenceJSON); got != "[]" {
		t.Fatalf("evidence_json = %s, want []", got)
	}
}
