package dcverify

// The client's side of the boundary, driven against the real binary.
//
// adversarial_test.go proves the client refuses a verifier that misbehaves; it
// can prove that with fakes because a fake is exactly as good as the real thing
// at misbehaving. It cannot prove the other direction. Every claim the client
// makes *about* dcverify — that both findings gates run on every check, that
// scope classification partitions the diff, that a coverage profile is read —
// is a claim about a program in another language that this build does not
// compile, and the only evidence for it is running that program.
//
// Without these, the client's comments would be the sort of documentation that
// stays confidently true after the thing it describes has changed.

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// client builds the real binary and returns a client pointed at it.
func client(t *testing.T) *Client {
	t.Helper()
	c := New(testsupport.DCVerify(t), t.TempDir())
	c.Timeout = 60 * time.Second
	return c
}

// plantedDiff adds one blocking line per findings gate, so a single check
// exercises both.
//
// The AWS key is `AKIAIOSFODNN7EXAMPLE`, the example key AWS publishes in its
// own documentation for exactly this purpose. A test that needed a real
// credential to prove the credential scanner works would be the leak it is
// testing for.
//
// The stub is `todo!()`, one of the empty bodies the detector treats as
// blocking. A bare `// TODO` comment would not do: those are advisory by
// design, so the severity half of this test would be asserting the opposite of
// what it means to.
const plantedDiff = `diff --git a/src/handler.rs b/src/handler.rs
--- a/src/handler.rs
+++ b/src/handler.rs
@@ -1,0 +1,4 @@
+pub fn handle() -> Result<(), Error> {
+    let aws_key = "AKIAIOSFODNN7EXAMPLE";
+    todo!()
+}
`

// TestBothFindingsGatesRunOnEveryCheck is the evidence behind GatesRun.
//
// GatesRun names the secret scanner and the stub detector unconditionally, and
// a caller records that list as the rigor it applied. The justification is that
// `dcverify check` calls both on every parsed diff — which is a claim about
// rust/dc-verify, not about this package. If a gate there became conditional,
// GatesRun would keep naming it and the host would keep reporting a gate that
// no longer ran. This is the test that goes red instead.
func TestBothFindingsGatesRunOnEveryCheck(t *testing.T) {
	result, err := client(t).Check(context.Background(), Request{Diff: plantedDiff})
	if err != nil {
		t.Fatalf("check: %v", err)
	}

	byGate := map[string][]Finding{}
	for _, f := range result.Findings {
		byGate[f.Gate] = append(byGate[f.Gate], f)
	}
	for _, gate := range []string{GateSecretScan, GateStubDetection} {
		if len(byGate[gate]) == 0 {
			t.Errorf("gate %s reported nothing for a diff that plants one of its cases; "+
				"GatesRun() names it on every check: %+v", gate, result.Findings)
		}
	}

	// Both planted lines are the blocking kind — a credential and an empty
	// body. Severity is what decides whether the caller's gap blocks, so a gate
	// that ran but was demoted to advisory is the same failure as one that did
	// not run, arriving in a different disguise.
	for gate, findings := range byGate {
		blocking := false
		for _, f := range findings {
			if f.Blocking() {
				blocking = true
			}
		}
		if !blocking {
			t.Errorf("gate %s reported only advisory findings for a planted blocking case: %+v",
				gate, findings)
		}
	}

	// The secret must not come back in full. The finding travels into a gap,
	// the terminal and the session log, and a report that quotes the key has
	// copied the credential into three more places.
	for _, f := range byGate[GateSecretScan] {
		if strings.Contains(f.Evidence, "AKIAIOSFODNN7EXAMPLE") {
			t.Errorf("the secret finding quotes the key in full: %q", f.Evidence)
		}
	}
}

// TestScopeClassificationPartitionsTheDiff is the evidence behind validate's
// file-count rule, which refuses a reply whose in_scope and orphans do not add
// up to its file count.
//
// That rule is only safe if the binary really does put every changed file in
// exactly one of the two. If it ever grew a third bucket, the client would
// refuse every real reply — a rule that turns a working verifier into a broken
// one is worth holding to the actual behaviour rather than to a reading of it.
func TestScopeClassificationPartitionsTheDiff(t *testing.T) {
	const twoFileDiff = `diff --git a/src/planned.go b/src/planned.go
--- a/src/planned.go
+++ b/src/planned.go
@@ -1,0 +1,1 @@
+const a = 1
diff --git a/src/unplanned.go b/src/unplanned.go
--- a/src/unplanned.go
+++ b/src/unplanned.go
@@ -1,0 +1,1 @@
+const b = 2
`
	result, err := client(t).Check(context.Background(), Request{
		Diff:    twoFileDiff,
		Planned: []string{"src/planned.go", "src/never_touched.go"},
	})
	if err != nil {
		t.Fatalf("check: %v", err)
	}

	if result.Files != 2 {
		t.Fatalf("files=%d, want 2", result.Files)
	}
	if len(result.InScope) != 1 || result.InScope[0] != "src/planned.go" {
		t.Errorf("in_scope=%v, want the planned file", result.InScope)
	}
	if len(result.Orphans) != 1 || result.Orphans[0] != "src/unplanned.go" {
		t.Errorf("orphans=%v, want the unplanned file", result.Orphans)
	}
	if len(result.UntouchedPlanned) != 1 || result.UntouchedPlanned[0] != "src/never_touched.go" {
		t.Errorf("untouched_planned=%v, want the planned path no file matched", result.UntouchedPlanned)
	}
}

// TestACoverageProfileIsActuallyRead is the evidence behind Coverage.Measured.
//
// Measured is set from the request, which is what makes it honest — but only if
// supplying the path genuinely changes what the binary does. A --coverage flag
// the binary ignored would leave Measured reporting a measurement that never
// happened, and this package would have built the exact lie it exists to
// prevent, one layer up.
func TestACoverageProfileIsActuallyRead(t *testing.T) {
	const diff = `diff --git a/src/covered.go b/src/covered.go
--- a/src/covered.go
+++ b/src/covered.go
@@ -1,0 +1,2 @@
+const a = 1
+const b = 2
`
	c := client(t)

	// Without a profile: no gaps, and the file is unmeasured. This is the reply
	// that reads as "clean" to anyone looking only at gaps.
	unmeasured, err := c.Check(context.Background(), Request{Diff: diff})
	if err != nil {
		t.Fatalf("check without a profile: %v", err)
	}
	if got := unmeasured.Coverage(); got.Measured || len(got.Gaps) != 0 {
		t.Fatalf("measured=%v gaps=%v; no profile was supplied", got.Measured, got.Gaps)
	}
	if got := unmeasured.Coverage().Unmeasured; len(got) != 1 || got[0] != "src/covered.go" {
		t.Fatalf("unmeasured=%v, want the changed file", got)
	}

	// With a profile that covers line 1 and not line 2, the same diff earns a
	// gap naming line 2. Line-level, so a profile the binary merely opened and
	// discarded would not produce it.
	profile := filepath.Join(t.TempDir(), "lcov.info")
	if err := os.WriteFile(profile, []byte("SF:src/covered.go\nDA:1,1\nDA:2,0\nend_of_record\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	measured, err := c.Check(context.Background(), Request{Diff: diff, CoveragePath: profile})
	if err != nil {
		t.Fatalf("check with a profile: %v", err)
	}
	coverage := measured.Coverage()
	if !coverage.Measured {
		t.Fatal("a profile was supplied, so this is a measurement")
	}
	if len(coverage.Unmeasured) != 0 {
		t.Errorf("unmeasured=%v; the profile names this file", coverage.Unmeasured)
	}
	if len(coverage.Gaps) != 1 {
		t.Fatalf("gaps=%+v, want one", coverage.Gaps)
	}
	gap := coverage.Gaps[0]
	if gap.Path != "src/covered.go" || gap.AddedLines != 2 {
		t.Errorf("gap=%+v, want two added lines in src/covered.go", gap)
	}
	if len(gap.UncoveredLines) != 1 || gap.UncoveredLines[0] != 2 {
		t.Errorf("uncovered=%v, want only the line the profile records as unexecuted", gap.UncoveredLines)
	}
}

// TestAMissingCoverageProfileIsAnErrorNotAnUnmeasuredRun.
//
// A typo'd path must not degrade into "nothing was covered". That report is
// identical to one from a suite that genuinely ran nothing, and the caller
// would open a coverage gap against code whose tests were fine.
func TestAMissingCoverageProfileIsAnErrorNotAnUnmeasuredRun(t *testing.T) {
	missing := filepath.Join(t.TempDir(), "no-such-profile.info")
	result, err := client(t).Check(context.Background(), Request{
		Diff:         plantedDiff,
		CoveragePath: missing,
	})
	if err == nil {
		t.Fatalf("a coverage path that names no file must be an error, got %+v", result)
	}
	if !strings.Contains(err.Error(), "coverage") {
		t.Errorf("the error must name what could not be read: %v", err)
	}
}

// TestAnUnparseableDiffIsAnErrorNotACleanReport. The binary's own module
// comment names this as its central rule; the client is the half that has to
// not undo it. An exit-2 refusal read as "no findings" is a verifier that
// approves whatever it cannot parse.
func TestAnUnparseableDiffIsAnErrorNotACleanReport(t *testing.T) {
	result, err := client(t).Check(context.Background(), Request{
		Diff: "@@ this is not a unified diff @@\n+stray addition\n",
	})
	if err == nil {
		t.Fatalf("an unparseable diff must be an error, got %+v", result)
	}
	if result != nil {
		t.Fatalf("an error must not also carry a result: %+v", result)
	}
}

// TestAnEmptyDiffIsACleanReportRatherThanAFailure is the positive control for
// the test above: the client must not have made *every* unusual input an error.
// An empty diff is a legitimate thing to check — it is what a task with no work
// produces — and it reports zero files, not a failure.
func TestAnEmptyDiffIsACleanReportRatherThanAFailure(t *testing.T) {
	result, err := client(t).Check(context.Background(), Request{Diff: ""})
	if err != nil {
		t.Fatalf("an empty diff is a clean report, not a failure: %v", err)
	}
	if result.Files != 0 || len(result.Findings) != 0 {
		t.Errorf("files=%d findings=%+v, want an empty report", result.Files, result.Findings)
	}
}

// TestTheHealthProbeAcceptsTheRealBinary. Available refuses on identity and on
// schema version, and both are pinned constants; a positive control is what
// keeps them from being pinned to values nothing answers with.
func TestTheHealthProbeAcceptsTheRealBinary(t *testing.T) {
	if err := client(t).Available(context.Background()); err != nil {
		t.Fatalf("the real binary must pass its own health probe: %v", err)
	}
}
