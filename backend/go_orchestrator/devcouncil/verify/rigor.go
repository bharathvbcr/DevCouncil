package verify

// The host's half of the rigor layer.
//
// The gates themselves live in `dcverify` and are reached through dc/dcverify,
// which is the boundary that refuses to report a gate it did not run. This file
// owns what happens on this side of that boundary: turning findings into the
// gap vocabulary the repair loop already speaks, and — the part that matters
// more — making sure the report says which gates actually ran.
//
// Before this, `verify.Run` set `RigorApplied` to an empty slice and moved on.
// That was honest only by accident: an empty list read as "no rigor findings"
// just as easily as "no rigor", and the two surfaces reading it (MCP
// `verify_task` and `devcouncil verify --json`) could not tell them apart. The
// pairing below is the fix — a run either names the gates it applied or names
// the reason it applied none, never neither and never both.
//
// Scope classification is deliberately *not* consumed here. dcverify also
// splits the diff into in-scope and orphan files, but DetectOrphanDiffGaps
// already owns that question on this side and its gap ids are pinned by the
// goldens. Two producers of one finding is how the two answers come to
// disagree, so the planned list is still sent — a reply whose scope arrays were
// computed against nothing would be actively wrong — and only the findings and
// the coverage answer are read back.

import (
	"context"
	"os"
	"sort"
	"strings"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/dcverify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
)

// RigorClient resolves the dcverify binary for a repository, or returns nil
// when there is none to run.
//
// nil rather than a client pointed at a guessed name: a Client whose Binary is
// "dcverify" fails at exec time with a message about a path, once per verify,
// where a nil client produces one sentence naming what to install. The
// difference matters because this is the common state — dcverify is an optional
// component — and a boundary that is loud about being absent is what keeps its
// absence from reading as a pass.
//
// Discovery refuses a candidate inside the repository under analysis. See
// proc.LookPathOutside: repository contents are input to verification, and a
// build output or vendored script that happens to be named dcverify must not
// become the program that decides whether that repository's diff is clean.
func RigorClient(root string) *dcverify.Client {
	if override := strings.TrimSpace(os.Getenv(dcverify.BinaryEnv)); override != "" {
		// An operator who names a path means it, including a path inside the
		// repository — which is how this is developed and tested.
		return dcverify.New(override, root)
	}
	binary, err := proc.LookPathOutside("dcverify", root)
	if err != nil {
		return nil
	}
	return dcverify.New(binary, root)
}

// rigorOutcome is one rigor pass.
//
// applied and skippedReason are a pair: exactly one of them is populated.
// Anything else is the state this file exists to prevent, where a report claims
// gates it did not run or stays silent about gates it skipped.
type rigorOutcome struct {
	gaps    []Gap
	applied []string
	// skippedReason says why no gate ran, in terms an operator can act on.
	skippedReason string
	// The coverage half is tracked separately because it is a different fact:
	// the findings gates can run perfectly well on a change nobody measured.
	coverageMeasured      bool
	coverageSkippedReason string
}

// rigorSkipped builds the outcome for a pass that did not happen.
//
// Constructed through a function rather than assembled at each early return, so
// a skip cannot be written that forgets to say why. A skip with an empty reason
// is indistinguishable in every report from a clean pass.
func rigorSkipped(reason, coverageReason string) rigorOutcome {
	return rigorOutcome{
		applied:               []string{},
		skippedReason:         reason,
		coverageMeasured:      false,
		coverageSkippedReason: coverageReason,
	}
}

// runRigorGates spawns dcverify for one task's diff.
//
// It never returns a pass it did not earn. The three ways out are: the gates
// ran (applied names them), the gates were not configured (skippedReason says
// so), or the gates were configured and failed — which is a blocking gap, not a
// skip, because a verifier that is present and broken is a different situation
// from one that was never installed.
func runRigorGates(ctx context.Context, in Input, taskID string, planned []string) rigorOutcome {
	// Coverage's own reason is resolved first, because it is true regardless of
	// whether the findings gates run: there is nothing to measure in an empty
	// diff, and nothing to measure against without a profile.
	coverageReason := "coverage profile not supplied"
	if in.DiffEmpty || strings.TrimSpace(in.DiffContent) == "" {
		coverageReason = "no diff to measure"
	}

	if in.Rigor == nil {
		return rigorSkipped(
			"no dcverify binary configured; install it with `devcouncil install --only=dcverify` "+
				"or point "+dcverify.BinaryEnv+" at a build",
			coverageReason)
	}
	if in.DiffEmpty || strings.TrimSpace(in.DiffContent) == "" {
		// Not a failure. The gates read added lines, and there are none.
		return rigorSkipped("no diff to check", coverageReason)
	}

	result, err := in.Rigor.Check(ctx, dcverify.Request{
		Diff:         in.DiffContent,
		Planned:      planned,
		CoveragePath: in.CoveragePath,
	})
	if err != nil {
		// A configured verifier that could not answer. Reported as a blocking
		// gap rather than as a skip: the operator installed these gates, so a
		// run where the secret scanner silently did not happen must not come
		// back looking like a run where it happened and found nothing.
		return rigorOutcome{
			gaps: []Gap{{
				ID:       StableGapID(taskID, "RIGOR"),
				Severity: "high",
				GapType:  "rigor_check_unavailable",
				TaskID:   taskID,
				Description: "The rigor gates (stub detection, secret scanning, diff coverage) " +
					"were configured but could not run: " + err.Error(),
				Evidence: []string{"dcverify: " + err.Error()},
				RecommendedFix: "Repair or reinstall dcverify, then re-verify. Until it answers, " +
					"this diff has not been scanned for credentials or placeholders.",
				Blocking: true,
			}},
			applied: []string{},
			// Not a skippedReason: the gap carries the diagnosis, and setting
			// both would report the same failure twice in two vocabularies.
			skippedReason:         "",
			coverageMeasured:      false,
			coverageSkippedReason: coverageReason,
		}
	}

	out := rigorOutcome{
		gaps:             gapsFromFindings(taskID, result.Findings),
		applied:          result.GatesRun(),
		coverageMeasured: result.Coverage().Measured,
	}
	if out.coverageMeasured {
		out.gaps = append(out.gaps, gapsFromCoverage(taskID, result.Coverage())...)
	} else {
		out.coverageSkippedReason = coverageReason
	}
	return out
}

// gapsFromFindings translates dcverify's findings into the gap vocabulary.
//
// The severity a gate assigned decides whether the gap blocks. That mapping is
// the verifier's to make — it is the thing that knows a credential is worse
// than a TODO comment — so it is carried across rather than re-decided here.
func gapsFromFindings(taskID string, findings []dcverify.Finding) []Gap {
	gaps := make([]Gap, 0, len(findings))
	for _, f := range findings {
		gapType, severity, fix := "", "", ""
		switch f.Gate {
		case dcverify.GateSecretScan:
			gapType = "security_risk"
			// Critical regardless of anything else in the run: a credential in
			// a commit is compromised even after it is deleted, because the
			// object stays in the repository's history.
			severity = "critical"
			fix = "Remove the credential from the diff and rotate it; it is compromised " +
				"the moment this lands, and deleting the line later does not undo that."
		case dcverify.GateStubDetection:
			gapType = "stub_detected"
			severity = "high"
			fix = "Replace the placeholder with a real implementation before marking the task done."
			if !f.Blocking() {
				// An added TODO marker, which the gate raises as a note rather
				// than a refusal. Recording it at the blocking severity would
				// make the gate something people route around.
				severity = "low"
				fix = "Resolve or justify the marker before marking the task done."
			}
		default:
			// Unreachable: dc/dcverify refuses a reply carrying a gate it
			// cannot map, so this cannot be a finding silently dropped. The
			// branch exists so that a gate added there without a gap type here
			// fails visibly instead of vanishing.
			gapType = "rigor_check_unavailable"
			severity = "high"
			fix = "A rigor gate reported a finding this host has no gap type for; " +
				"map " + f.Gate + " in verify/rigor.go."
		}

		line := f.Line
		gaps = append(gaps, Gap{
			// The identity carries gate, path and line, so two findings in one
			// file stay two gaps rather than collapsing into one in
			// NormalizeGaps.
			ID:             StableGapID(taskID, strings.ToUpper(f.Gate), f.Gate+":"+f.Path+":"+itoa(line)),
			Severity:       severity,
			GapType:        gapType,
			TaskID:         taskID,
			Description:    f.Message,
			Evidence:       []string{f.Path + ":" + itoa(line) + ": " + f.Evidence},
			RecommendedFix: fix,
			Blocking:       f.Blocking(),
			File:           filePtr(f.Path),
			Line:           &line,
		})
	}
	return gaps
}

// gapsFromCoverage reports changed files whose added lines were not executed.
//
// Non-blocking, which is the documented default: the gate is signal-first, so
// an unexercised diff is surfaced in gaps and next_actions without failing the
// task. Promoting it under `verification.diff_coverage.enforce` is not wired
// into this host, and this file does not pretend otherwise — a gap that claims
// to be enforcing when nothing reads the flag would be worse than one that
// says plainly what it is.
//
// Only called when a profile was supplied. Without one the verifier reports no
// gaps at all, and emitting nothing from that would be reporting a fully
// exercised diff for a run that measured nothing.
func gapsFromCoverage(taskID string, coverage dcverify.Coverage) []Gap {
	gaps := make([]Gap, 0, len(coverage.Gaps))
	for _, gap := range coverage.Gaps {
		lines := make([]string, 0, len(gap.UncoveredLines))
		for _, line := range gap.UncoveredLines {
			lines = append(lines, itoa(line))
		}
		first := gap.UncoveredLines[0]
		gaps = append(gaps, Gap{
			ID:       StableGapID(taskID, "COVERAGE", "coverage:"+gap.Path),
			Severity: "medium",
			GapType:  "diff_not_exercised",
			TaskID:   taskID,
			Description: gap.Path + ": " + itoa(len(gap.UncoveredLines)) + " of " +
				itoa(gap.AddedLines) + " added lines were not executed by the verification run.",
			Evidence: []string{gap.Path + " uncovered lines: " + strings.Join(lines, ", ")},
			RecommendedFix: "Add or extend a test that executes the changed lines in " +
				gap.Path + ", then re-verify.",
			Blocking: false,
			File:     filePtr(gap.Path),
			Line:     &first,
		})
	}
	// Sorted so two runs over the same diff produce the same report. The
	// verifier walks files in diff order, which is git's, and a gap list that
	// reorders between runs makes every diff of two reports noise.
	sort.SliceStable(gaps, func(i, j int) bool { return *gaps[i].File < *gaps[j].File })
	return gaps
}
