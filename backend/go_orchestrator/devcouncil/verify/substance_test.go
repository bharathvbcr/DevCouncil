package verify_test

// The substance measurement's host wiring.
//
// dc-verify owns the classification and its threshold; what is left to prove
// here is that the measurement survives the process boundary and becomes
// something the repair loop can read — and, just as important, that it never
// blocks. A pure refactor measures low by construction, so a blocking
// low_substance gap would fail correct work, and a gate that fails correct work
// gets switched off.

import (
	"context"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/dcverify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// lockfileDiff is the shape every other gate passes cleanly: twenty-five added
// lines, none of them written by anyone, no marker and no credential.
const lockfileDiff = `diff --git a/Cargo.lock b/Cargo.lock
--- a/Cargo.lock
+++ b/Cargo.lock
@@ -0,0 +1,25 @@
+[[package]]
+name = "anyhow"
+version = "1.0.100"
+source = "registry+https://github.com/rust-lang/crates.io-index"
+checksum = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
+
+[[package]]
+name = "bitflags"
+version = "2.9.0"
+source = "registry+https://github.com/rust-lang/crates.io-index"
+checksum = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
+
+[[package]]
+name = "cfg-if"
+version = "1.0.4"
+source = "registry+https://github.com/rust-lang/crates.io-index"
+checksum = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
+
+[[package]]
+name = "libc"
+version = "0.2.189"
+source = "registry+https://github.com/rust-lang/crates.io-index"
+checksum = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
+
+[[package]]
`

// realWorkDiff is the control: the same size, written by hand.
const realWorkDiff = `diff --git a/src/budget.rs b/src/budget.rs
--- a/src/budget.rs
+++ b/src/budget.rs
@@ -0,0 +1,22 @@
+/// Bounds how long one verification pass may run.
+pub struct Budget {
+    deadline: Instant,
+    spent: Duration,
+}
+
+impl Budget {
+    pub fn starting_now(limit: Duration) -> Self {
+        Budget { deadline: Instant::now() + limit, spent: Duration::ZERO }
+    }
+
+    pub fn remaining(&self) -> Duration {
+        self.deadline.saturating_duration_since(Instant::now())
+    }
+
+    pub fn charge(&mut self, cost: Duration) -> Result<(), Exhausted> {
+        self.spent += cost;
+        if self.remaining().is_zero() {
+            return Err(Exhausted { spent: self.spent });
+        }
+        Ok(())
+    }
+}
`

// substanceInput builds a run whose only changed file is planned, so no
// orphan-diff gap can be confused for the one under test.
func substanceInput(t *testing.T, id, path, diff, gateMode string) verify.Input {
	t.Helper()
	return verify.Input{
		Task: &store.Task{
			ID:         id,
			Difficulty: "easy",
			PlannedFiles: []dc.PlannedFile{
				{Path: path, AllowedChange: dc.ChangeModify},
			},
		},
		GateMode:     gateMode,
		ChangedFiles: []string{path},
		DiffContent:  diff,
		DiffEmpty:    false,
		WorkPresent:  true,
		Rigor:        dcverify.New(testsupport.DCVerify(t), t.TempDir()),
	}
}

// TestADiffThatPassesEveryGateWithNoNewWorkIsStillReported is the whole point
// of the measurement.
//
// Before it, this diff produced `findings: []`, no coverage gap, and a clean
// verdict — which reads as "the gates ran and found nothing", and which a
// caller reasonably reads as work having been done.
func TestADiffThatPassesEveryGateWithNoNewWorkIsStillReported(t *testing.T) {
	gaps, meta := verify.Run(context.Background(),
		substanceInput(t, "TASK-SUB-LOW", "Cargo.lock", lockfileDiff, "enforce"))
	byType := gapsByType(gaps)

	found := byType["low_substance"]
	if len(found) != 1 {
		t.Fatalf("want exactly one low_substance gap for a regenerated lockfile, got %d: %+v",
			len(found), gaps)
	}
	gap := found[0]

	// Never blocking, in enforce mode included. A refactor and a lockfile
	// refresh are both legitimate and both measure low.
	if gap.Blocking {
		t.Errorf("the low_substance gap blocks; a pure refactor measures low by "+
			"construction, so blocking on it fails correct work: %+v", gap)
	}

	// The evidence must carry every class count, not just the verdict. A gap
	// that says "low" without the numbers cannot be checked against the diff it
	// describes, and the reader's only option is to believe it.
	if len(gap.Evidence) == 0 {
		t.Fatalf("the low_substance gap carries no evidence: %+v", gap)
	}
	for _, want := range []string{"substantive", "trivial", "moved", "repeated", "generated"} {
		if !strings.Contains(gap.Evidence[0], want) {
			t.Errorf("evidence %q omits the %s count", gap.Evidence[0], want)
		}
	}
	if !strings.Contains(gap.Evidence[0], "generated 25") {
		t.Errorf("evidence %q does not attribute all 25 added lines to the generated "+
			"class, which is what makes this diff low", gap.Evidence[0])
	}

	// And the gate must be named as applied. A measurement that ran and is not
	// listed is the same reporting failure as one that is listed and did not.
	result := verify.ToMCP("TASK-SUB-LOW", gaps, meta)
	if !slicesContains(result.RigorApplied, dcverify.GateSubstance) {
		t.Errorf("rigor_applied=%v omits the substance gate, which always runs",
			result.RigorApplied)
	}
}

// TestOrdinaryWorkRaisesNoSubstanceGap is the false-positive half.
//
// Without it every assertion above is satisfied by a measurement that reports
// `low` for everything, which would be worse than no measurement: it would
// train a reader to skip the row.
func TestOrdinaryWorkRaisesNoSubstanceGap(t *testing.T) {
	gaps, _ := verify.Run(context.Background(),
		substanceInput(t, "TASK-SUB-OK", "src/budget.rs", realWorkDiff, "enforce"))
	if found := gapsByType(gaps)["low_substance"]; len(found) != 0 {
		t.Errorf("ordinary new code raised %d low_substance gap(s): %+v", len(found), found)
	}
}

// TestASmallDiffIsNotJudgedAtAll pins the floor.
//
// A three-line change scores zero and means nothing by it. Reporting a gap
// there would put a row in almost every small task's report, which is how a
// signal becomes noise.
func TestASmallDiffIsNotJudgedAtAll(t *testing.T) {
	const tiny = `diff --git a/src/budget.rs b/src/budget.rs
--- a/src/budget.rs
+++ b/src/budget.rs
@@ -0,0 +1,2 @@
+    }
+}
`
	gaps, _ := verify.Run(context.Background(),
		substanceInput(t, "TASK-SUB-TINY", "src/budget.rs", tiny, "enforce"))
	if found := gapsByType(gaps)["low_substance"]; len(found) != 0 {
		t.Errorf("a two-line diff was judged: %+v", found)
	}
}
