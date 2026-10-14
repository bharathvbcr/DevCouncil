package gate

import (
	"runtime"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/grants"
)

// liveHeap is the heap still reachable after a full collection.
func liveHeap() uint64 {
	runtime.GC()
	runtime.GC()
	var m runtime.MemStats
	runtime.ReadMemStats(&m)
	return m.HeapAlloc
}

// A long-lived host (devcouncil mcp, manvi serve) asks the gate about every
// write for the life of the process. What the gate keeps for its run summary
// must not grow with that: memory after a million decisions is what it was
// after a hundred thousand, while the summary's counts stay exact.
func TestTheDecisionLogIsFlatAfterAMillionWrites(t *testing.T) {
	if testing.Short() {
		t.Skip("a million evaluations")
	}
	const warm, total = 100_000, 1_000_000
	g := newGate(t, nil)
	task := testTask()
	evaluate := func(i int) {
		path := "src/calc.go"
		if i%4 == 0 {
			path = "src/legacy/old.go" // forbidden: a denial
		}
		if _, err := g.EvaluateWrite(path, task, dc.OpWrite); err != nil {
			t.Fatalf("evaluate %d: %v", i, err)
		}
	}
	for i := 0; i < warm; i++ {
		evaluate(i)
	}
	atWarm := liveHeap()
	for i := warm; i < total; i++ {
		evaluate(i)
	}
	atTotal := liveHeap()

	// 900k more decisions held in a slice is well over 100 MiB; a bound keeps
	// the difference to noise.
	const slack = 8 << 20
	if atTotal > atWarm+slack {
		t.Errorf("live heap grew from %d to %d bytes over %d further decisions", atWarm, atTotal, total-warm)
	}
	if kept := len(g.Decisions()); kept >= total {
		t.Errorf("the gate kept all %d decisions", kept)
	}

	rep := g.Report()
	if rep.Total != total || rep.Denied != total/4 || rep.Clean != total-total/4 {
		t.Errorf("counts are not exact: total %d denied %d clean %d", rep.Total, rep.Denied, rep.Clean)
	}
	if rep.DecisionsSampled != maxRecordedDecisions || rep.DecisionsDropped != total-maxRecordedDecisions {
		t.Errorf("sample %d dropped %d, want %d and %d", rep.DecisionsSampled, rep.DecisionsDropped,
			maxRecordedDecisions, total-maxRecordedDecisions)
	}
}

// Once the ring has wrapped, Decisions is the most recent window in order, the
// report says the sample is capped, and a grant applied by a decision that fell
// out of the window is still not reported as issued-and-unused.
func TestADroppedDecisionIsCountedAndItsGrantStaysApplied(t *testing.T) {
	g := newGate(t, nil)
	blocked, _ := g.EvaluateWrite("internal/other.go", testTask(), dc.OpWrite)
	if _, err := g.RequestOverride(blocked,
		grants.Grantor{Authority: grants.Human, ID: "operator"}, "the plan omitted it", ""); err != nil {
		t.Fatal(err)
	}
	if d, _ := g.EvaluateWrite("internal/other.go", testTask(), dc.OpWrite); d.GrantID == "" {
		t.Fatalf("the grant should have cleared this: %+v", d)
	}
	if rep := g.Report(); rep.DecisionsDropped != 0 || rep.DecisionsSampled != 2 || len(rep.GrantLines) != 1 {
		t.Fatalf("before the window fills nothing is dropped: %+v", rep)
	}

	const extra = maxRecordedDecisions + 10
	for i := 0; i < extra; i++ {
		if _, err := g.EvaluateWrite("src/calc.go", testTask(), dc.OpWrite); err != nil {
			t.Fatal(err)
		}
	}
	// One more, distinguishable, so the order of the window can be checked.
	if _, err := g.EvaluateWrite("src/legacy/last.go", testTask(), dc.OpWrite); err != nil {
		t.Fatal(err)
	}

	kept := g.Decisions()
	if len(kept) != maxRecordedDecisions {
		t.Fatalf("kept %d decisions, want %d", len(kept), maxRecordedDecisions)
	}
	if last := kept[len(kept)-1]; last.Target != "src/legacy/last.go" {
		t.Errorf("the newest decision is not last: %+v", last)
	}
	for _, d := range kept[:len(kept)-1] {
		if d.Target != "src/calc.go" {
			t.Fatalf("a decision older than the window survived: %+v", d)
		}
	}

	rep := g.Report()
	total := 2 + extra + 1
	if rep.Total != total || rep.Denied != 2 || rep.Granted != 1 || rep.Clean != extra {
		t.Errorf("counts are not exact: %+v", rep)
	}
	if rep.DecisionsDropped != total-maxRecordedDecisions {
		t.Errorf("dropped %d, want %d", rep.DecisionsDropped, total-maxRecordedDecisions)
	}
	if len(rep.IssuedGrantLines) != 0 {
		t.Errorf("a grant applied before the window is not unused: %v", rep.IssuedGrantLines)
	}
}
