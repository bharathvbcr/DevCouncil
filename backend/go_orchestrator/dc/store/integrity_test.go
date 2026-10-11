package store

import (
	"context"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"testing"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

func acquireT1(c *Client) (*Lease, error) {
	return c.Acquire(context.Background(), AcquireRequest{TaskID: "T1", Owner: "o", TTL: 15 * time.Minute})
}

// A warm pooled child holds the database it opened. When the path is deleted
// the pool must refuse rather than keep answering from the unlinked file.
func TestAWarmPoolRefusesAfterTheDatabaseIsDeleted(t *testing.T) {
	c := client(t)
	t.Cleanup(c.Close)
	if _, err := c.ActiveLeases(context.Background()); err != nil { // creates the store
		t.Fatal(err)
	}
	if err := c.Available(context.Background()); err != nil {
		t.Fatalf("a fresh store is unavailable: %v", err)
	}
	if err := os.Remove(c.DB); err != nil {
		t.Fatal(err)
	}
	if err := c.Available(context.Background()); err == nil {
		t.Fatal("a warm pool reported a deleted database available")
	}
}

// Replacing the file leaves the warm child on the old inode. Two clients must
// not each lease the same task, one per file.
func TestAWarmPoolNeverLeasesTheSameTaskOnAReplacedDatabase(t *testing.T) {
	warm := client(t)
	t.Cleanup(warm.Close)
	if _, err := warm.ActiveLeases(context.Background()); err != nil { // creates the store
		t.Fatal(err)
	}
	if err := warm.Available(context.Background()); err != nil {
		t.Fatal(err)
	}

	// Another client builds a fresh database and renames it over the path, so
	// the warm child keeps the old, now unlinked, inode.
	fresh := New(warm.Binary, filepath.Join(filepath.Dir(warm.DB), "fresh.sqlite"))
	if _, err := fresh.ActiveLeases(context.Background()); err != nil {
		t.Fatal(err)
	}
	fresh.Close()
	// The journal files go with it: they are named after the path, and left in
	// place the old and new databases would share one WAL index.
	for _, suffix := range []string{"-wal", "-shm"} {
		if err := os.Remove(warm.DB + suffix); err != nil && !errors.Is(err, os.ErrNotExist) {
			t.Fatal(err)
		}
	}
	if err := os.Rename(fresh.DB, warm.DB); err != nil {
		t.Fatal(err)
	}

	second := New(warm.Binary, warm.DB)
	t.Cleanup(second.Close)
	if _, err := acquireT1(second); err != nil {
		t.Fatalf("the first client to reach the new file must lease the task: %v", err)
	}
	lease, err := acquireT1(warm)
	var held *Conflict
	if !errors.As(err, &held) {
		t.Fatalf("the warm pool leased a task already leased on the live database: lease=%+v err=%v", lease, err)
	}
}

func TestJSONSpaceIsTheOnlyTrimmedWhitespace(t *testing.T) {
	if got := string(trimJSONSpace([]byte(" \t\r\n[]\n "))); got != "[]" {
		t.Fatalf("JSON whitespace not trimmed: %q", got)
	}
	for _, padded := range []string{"\f[]", "[]\f", " []", "[] "} {
		if got := string(trimJSONSpace([]byte(padded))); got != padded {
			t.Fatalf("%q was trimmed to %q; only JSON whitespace may be", padded, got)
		}
	}
}

// The store reports a malformed stored evidence column beside the row; the
// client keeps the gap (so a blocking one is not hidden) and carries the reason.
func TestAMalformedStoredEvidenceKeepsTheBlockingGapAndCarriesTheReason(t *testing.T) {
	c := client(t)
	t.Cleanup(c.Close)
	ctx := context.Background()
	var gaps []GapRow
	for _, id := range []string{"G1", "G2"} {
		gaps = append(gaps, GapRow{ID: id, Severity: "high", GapType: "x", TaskID: "T1",
			Description: "d", RecommendedFix: "f", Blocking: true, EvidenceJSON: []byte(`[]`)})
	}
	if err := c.GapsReplace(ctx, "T1", gaps); err != nil {
		t.Fatal(err)
	}
	plant := exec.Command(testsupport.Tool(t, "sqlite3"), c.DB,
		`UPDATE gaps SET evidence_json='[]}],"gaps":[],"x":[{"q":0' WHERE id='G2'`)
	if out, err := plant.CombinedOutput(); err != nil {
		t.Fatalf("planting: %v %s", err, out)
	}
	rows, _, err := c.Gaps(ctx, "T1")
	if err != nil {
		t.Fatalf("gaps: %v", err)
	}
	if len(rows) != 2 {
		t.Fatalf("a planted row hid a gap: got %d rows", len(rows))
	}
	for _, r := range rows {
		if r.ID == "G2" && r.EvidenceError == "" {
			t.Fatal("the malformed evidence was not reported")
		}
		if r.ID == "G1" && (!r.Blocking || r.EvidenceError != "") {
			t.Fatalf("the blocking gap was disturbed: %+v", r)
		}
	}
}
