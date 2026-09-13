package verify_test

// The host's half of the rigor wiring, driven against the real dcverify.
//
// dc/dcverify proves the boundary refuses a verifier that misbehaves. What is
// left to prove is the thing TASK-P7-1 was opened for: that this host actually
// spawns it, that the findings become gaps the repair loop can route, and —
// above all — that a report never claims a gate it did not run.
//
// Every assertion below is about that last property in one form or another,
// because it is the one whose failure mode is silent. A missing gap is visible
// the first time someone looks for it; a `rigor_applied: []` that means "no
// verifier ran" while reading as "the gates found nothing" is not.

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/dcverify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// plantedDiff carries one blocking case for each findings gate. The AWS key is
// the example key AWS publishes for this purpose; `todo!()` is one of the empty
// bodies the stub detector treats as blocking rather than advisory.
const plantedDiff = `diff --git a/src/handler.rs b/src/handler.rs
--- a/src/handler.rs
+++ b/src/handler.rs
@@ -1,0 +1,4 @@
+pub fn handle() -> Result<(), Error> {
+    let aws_key = "AKIAIOSFODNN7EXAMPLE";
+    todo!()
+}
`

// plantedTask is the task that diff belongs to, planned so the file is in
// scope: an orphan-diff gap would pass through the same gap list and make the
// assertions below ambiguous about which check produced what.
func plantedTask(id string) *store.Task {
	return &store.Task{
		ID:         id,
		Difficulty: "easy",
		PlannedFiles: []dc.PlannedFile{
			{Path: "src/handler.rs", AllowedChange: dc.ChangeModify},
		},
	}
}

// withRigor builds the Input for a run whose gates really execute.
func withRigor(t *testing.T, id, gateMode string) verify.Input {
	t.Helper()
	return verify.Input{
		Task:         plantedTask(id),
		GateMode:     gateMode,
		ChangedFiles: []string{"src/handler.rs"},
		DiffContent:  plantedDiff,
		DiffEmpty:    false,
		WorkPresent:  true,
		Rigor:        dcverify.New(testsupport.DCVerify(t), t.TempDir()),
	}
}

func gapsByType(gaps []verify.Gap) map[string][]verify.Gap {
	out := map[string][]verify.Gap{}
	for _, g := range gaps {
		out[g.GapType] = append(out[g.GapType], g)
	}
	return out
}

// TestTheRigorGatesActuallyRun is TASK-P7-1's acceptance in one test.
//
// Before this wiring the host recorded `rigor_applied: []` for every run while
// documenting stub, secret and coverage gates, so a diff carrying a live
// credential verified clean.
func TestTheRigorGatesActuallyRun(t *testing.T) {
	gaps, meta := verify.Run(context.Background(), withRigor(t, "TASK-RIGOR", "enforce"))
	result := verify.ToMCP("TASK-RIGOR", gaps, meta)

	if len(result.RigorApplied) == 0 {
		t.Fatalf("rigor_applied is empty for a run whose gates were configured and ran: %+v", result)
	}
	for _, gate := range []string{dcverify.GateSecretScan, dcverify.GateStubDetection} {
		if !slicesContains(result.RigorApplied, gate) {
			t.Errorf("rigor_applied=%v omits %s", result.RigorApplied, gate)
		}
	}
	// Coverage did not run — no profile was supplied — so it must not appear in
	// the list of gates that did.
	if slicesContains(result.RigorApplied, dcverify.GateDiffCoverage) {
		t.Errorf("rigor_applied=%v names the coverage gate for a run with no profile",
			result.RigorApplied)
	}
	if result.CoverageMeasured {
		t.Error("coverage_measured is true for a run with no coverage profile")
	}
	if result.CoverageSkippedReason == "" {
		t.Error("unmeasured coverage needs a reason")
	}
	// Gates ran, so there is nothing to explain away.
	if result.RigorSkippedReason != "" {
		t.Errorf("rigor_skipped_reason=%q for a run whose gates ran", result.RigorSkippedReason)
	}

	byType := gapsByType(gaps)
	secrets := byType["security_risk"]
	if len(secrets) != 1 {
		t.Fatalf("security_risk gaps=%+v, want the planted credential", secrets)
	}
	if !secrets[0].Blocking || secrets[0].Severity != "critical" {
		t.Errorf("a credential in the diff must be a critical blocking gap: %+v", secrets[0])
	}
	if secrets[0].File == nil || *secrets[0].File != "src/handler.rs" {
		t.Errorf("the gap must name the file: %+v", secrets[0])
	}
	if secrets[0].Line == nil || *secrets[0].Line != 2 {
		t.Errorf("the gap must name the line the credential is on: %+v", secrets[0].Line)
	}
	// The gap travels into the store, the terminal and the session log. A
	// report that quotes the key has copied the credential into all three.
	for _, e := range secrets[0].Evidence {
		if strings.Contains(e, "AKIAIOSFODNN7EXAMPLE") {
			t.Errorf("the gap quotes the credential in full: %q", e)
		}
	}

	stubs := byType["stub_detected"]
	if len(stubs) != 1 {
		t.Fatalf("stub_detected gaps=%+v, want the planted empty body", stubs)
	}
	if !stubs[0].Blocking {
		t.Errorf("an empty body is a blocking stub: %+v", stubs[0])
	}

	// And the whole point: the task must not verify.
	if result.Passed || result.Status != "blocked" {
		t.Fatalf("a diff carrying a credential and a stub must not pass: status=%s passed=%v",
			result.Status, result.Passed)
	}
}

// TestRigorFindingsRouteToRepairActions. A gap the repair loop cannot categorise
// falls through to "review", which tells an agent nothing about what to do.
func TestRigorFindingsRouteToRepairActions(t *testing.T) {
	gaps, meta := verify.Run(context.Background(), withRigor(t, "TASK-ROUTE", "enforce"))
	result := verify.ToMCP("TASK-ROUTE", gaps, meta)

	want := map[string]string{"security_risk": "security", "stub_detected": "fix_code"}
	seen := map[string]string{}
	for _, a := range append(result.NextActions, result.AdvisoryActions...) {
		if _, interesting := want[a.GapType]; interesting {
			seen[a.GapType] = a.Category
			if strings.TrimSpace(a.Action) == "" {
				t.Errorf("%s carries no action text", a.GapType)
			}
		}
	}
	for gapType, category := range want {
		if seen[gapType] != category {
			t.Errorf("%s routed to category %q, want %q", gapType, seen[gapType], category)
		}
	}
}

// TestAnUnconfiguredVerifierSaysSoRatherThanReportingClean is the honesty
// invariant at the most common state: dcverify is an optional component, so
// most repositories run without it.
//
// `rigor_applied: []` is what this host reported for every run before the
// wiring landed, and it is still what it reports here — but now it cannot be
// read as "the gates found nothing", because a reason travels beside it.
func TestAnUnconfiguredVerifierSaysSoRatherThanReportingClean(t *testing.T) {
	in := withRigor(t, "TASK-NORIGOR", "enforce")
	in.Rigor = nil

	gaps, meta := verify.Run(context.Background(), in)
	result := verify.ToMCP("TASK-NORIGOR", gaps, meta)

	if len(result.RigorApplied) != 0 {
		t.Fatalf("rigor_applied=%v for a run with no verifier", result.RigorApplied)
	}
	if result.RigorSkippedReason == "" {
		t.Fatal("an empty rigor_applied with no reason beside it reads as a clean rigor pass; " +
			"that ambiguity is what this pair exists to remove")
	}
	// The reason has to be actionable, not merely present.
	if !strings.Contains(result.RigorSkippedReason, "dcverify") {
		t.Errorf("rigor_skipped_reason=%q does not name what is missing", result.RigorSkippedReason)
	}
	// No verifier ran, so no rigor gap may appear — the reason is the report.
	for _, g := range gaps {
		if g.GapType == "security_risk" || g.GapType == "stub_detected" {
			t.Errorf("a gap from a gate that never ran: %+v", g)
		}
	}
}

// TestAVerifierThatCannotRunIsABlockingGapNotASkip is the case the rest of the
// discipline turns on: the operator installed these gates, and this run did not
// get them.
//
// A skip here would be the exact failure the harness's policy layer exists to
// prevent — a check that could not run reporting the same thing as a check that
// ran and passed — committed by the layer that enforces it.
func TestAVerifierThatCannotRunIsABlockingGapNotASkip(t *testing.T) {
	broken := filepath.Join(t.TempDir(), "broken-dcverify")
	// #nosec G306 -- an executable stand-in; the execute bit is the point.
	if err := os.WriteFile(broken, []byte("#!/bin/sh\nexit 3\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	in := withRigor(t, "TASK-BROKEN", "enforce")
	in.Rigor = dcverify.New(broken, t.TempDir())

	gaps, meta := verify.Run(context.Background(), in)
	result := verify.ToMCP("TASK-BROKEN", gaps, meta)

	if len(result.RigorApplied) != 0 {
		t.Errorf("rigor_applied=%v for a verifier that failed", result.RigorApplied)
	}
	unavailable := gapsByType(gaps)["rigor_check_unavailable"]
	if len(unavailable) != 1 {
		t.Fatalf("gaps=%+v, want one rigor_check_unavailable", gaps)
	}
	if !unavailable[0].Blocking {
		t.Error("a configured verifier that could not run must block")
	}
	if result.Passed || result.Status != "blocked" {
		t.Fatalf("a run whose credential scan did not happen must not verify: status=%s passed=%v",
			result.Status, result.Passed)
	}

	// And it must stay blocking under advisory, where an ordinary quality gap
	// is demoted. This is the absence of evidence about credentials, not a
	// finding an operator can weigh.
	_, advisoryPassed := verify.StatusFromGaps(gaps, "advisory")
	if advisoryPassed {
		t.Error("advisory mode demoted a rigor gate that could not run")
	}
}

// TestAnEmptyDiffSkipsTheGatesWithoutClaimingThem. The gates read added lines;
// a task with no work has none. That is a legitimate skip and not a failure —
// but it is still a skip, and must not be recorded as gates applied.
func TestAnEmptyDiffSkipsTheGatesWithoutClaimingThem(t *testing.T) {
	in := withRigor(t, "TASK-EMPTY", "enforce")
	in.DiffContent, in.DiffEmpty, in.ChangedFiles, in.WorkPresent = "", true, nil, false

	gaps, meta := verify.Run(context.Background(), in)
	result := verify.ToMCP("TASK-EMPTY", gaps, meta)

	if len(result.RigorApplied) != 0 {
		t.Errorf("rigor_applied=%v for an empty diff", result.RigorApplied)
	}
	if result.RigorSkippedReason == "" {
		t.Error("a skipped rigor pass needs a reason")
	}
	// The pre-existing coverage wording is unchanged for this case, which is
	// what the leased/CLI goldens pin.
	if result.CoverageSkippedReason != "no diff to measure" {
		t.Errorf("coverage_skipped_reason=%q", result.CoverageSkippedReason)
	}
}

// TestACoverageProfileMeasuresTheDiff is the third gate, which is the one that
// only runs when it is given something to measure.
func TestACoverageProfileMeasuresTheDiff(t *testing.T) {
	// Two added lines in a file the profile covers only halfway. `handler.rs`
	// carries the planted findings too, so this also proves the two halves
	// coexist in one report.
	profile := filepath.Join(t.TempDir(), "lcov.info")
	lcov := "SF:src/handler.rs\nDA:1,1\nDA:2,0\nDA:3,0\nDA:4,1\nend_of_record\n"
	if err := os.WriteFile(profile, []byte(lcov), 0o600); err != nil {
		t.Fatal(err)
	}
	in := withRigor(t, "TASK-COVER", "enforce")
	in.CoveragePath = profile

	gaps, meta := verify.Run(context.Background(), in)
	result := verify.ToMCP("TASK-COVER", gaps, meta)

	if !result.CoverageMeasured {
		t.Fatal("a profile was supplied, so coverage was measured")
	}
	if result.CoverageSkippedReason != "" {
		t.Errorf("coverage_skipped_reason=%q for a measured run", result.CoverageSkippedReason)
	}
	if !slicesContains(result.RigorApplied, dcverify.GateDiffCoverage) {
		t.Errorf("rigor_applied=%v omits the coverage gate that ran", result.RigorApplied)
	}

	unexercised := gapsByType(gaps)["diff_not_exercised"]
	if len(unexercised) != 1 {
		t.Fatalf("diff_not_exercised gaps=%+v, want one for the half-covered file", unexercised)
	}
	// Signal-first by documented design: surfaced, not blocking.
	if unexercised[0].Blocking {
		t.Error("diff coverage is signal-first; it must not block on its own")
	}
	if !strings.Contains(unexercised[0].Evidence[0], "src/handler.rs") {
		t.Errorf("the gap must name the file: %+v", unexercised[0].Evidence)
	}
}

// TestTheCLIPayloadCarriesTheSameRigorAccount. Two surfaces read this — MCP
// `verify_task` and `devcouncil verify --json` — and an operator reading the
// CLI must not see a cleaner story than the agent reading MCP.
func TestTheCLIPayloadCarriesTheSameRigorAccount(t *testing.T) {
	in := withRigor(t, "TASK-CLIRIGOR", "enforce")
	in.Rigor = nil

	gaps, meta := verify.Run(context.Background(), in)
	mcp := verify.ToMCP("TASK-CLIRIGOR", gaps, meta)
	cli := verify.ToCLITask("TASK-CLIRIGOR", gaps, meta)

	if cli.RigorSkippedReason != mcp.RigorSkippedReason {
		t.Errorf("cli reason=%q, mcp reason=%q", cli.RigorSkippedReason, mcp.RigorSkippedReason)
	}
	if len(cli.RigorApplied) != len(mcp.RigorApplied) {
		t.Errorf("cli rigor_applied=%v, mcp=%v", cli.RigorApplied, mcp.RigorApplied)
	}
}

func slicesContains(haystack []string, needle string) bool {
	for _, item := range haystack {
		if item == needle {
			return true
		}
	}
	return false
}
