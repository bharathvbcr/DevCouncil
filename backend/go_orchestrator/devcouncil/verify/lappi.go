package verify

// The Lappi seam (Lappi-decision docs/caller-contract.md §3, "admission only").
//
// After the rigor gates, and only when DEVCOUNCIL_LAPPI_ASK=1, each changed
// file is put to the Lappi agent as a code.defect_class request. A model answer
// naming a defect may add one advisory, non-blocking gap for that file. That
// is all Lappi can do here:
//
//   - it never removes, rewrites or demotes another gap (appendLappiAdvisory
//     only appends, and every gap it appends has a type no other producer uses,
//     so NormalizeGaps cannot fold it into one);
//   - it never sets Blocking, RequirementID or AcceptanceCriterionID, so it is
//     never in MayDemote's path and never changes StatusFromGaps, which reads
//     only blocking gaps;
//   - it is never sent to dcverify, whose report refuses gates it does not
//     know.
//
// Today every such request is refused (calibration_entry_missing on the span
// slot), so no gap is added and a verify run is unchanged.

import (
	"context"
	"fmt"
	"os"
	"strconv"
	"strings"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/lappi"
)

// LappiAdvisoryPrefix is the gap-type prefix every Lappi gap carries.
const LappiAdvisoryPrefix = "advisory_lappi_"

// GapTypeLappiDefectClass is the one Lappi gap type.
const GapTypeLappiDefectClass = LappiAdvisoryPrefix + "defect_class"

// LappiAsker reads the Lappi settings from the environment. Nil — the default —
// means Lappi is neither asked nor recorded. A setting that is on but cannot be
// honoured (no HOME, a bad socket path) is reported on stderr and treated as
// off: Lappi is advisory, so its misconfiguration must not change a verify.
func LappiAsker() *lappi.Asker {
	asker, err := lappi.FromEnvironment(os.Getenv, os.Stderr)
	if err != nil {
		// stderr is the diagnostic stream for both the CLI and the MCP host
		// (stdout carries the protocol); a write failure there has no further
		// place to go.
		_, _ = fmt.Fprintf(os.Stderr, "devcouncil: lappi is enabled but cannot run: %v\n", err)
		return nil
	}
	return asker
}

// runLappi asks about the run's diff and returns the advisory gaps it earned.
// gaps is the run's account so far; it is read to record the run's own
// decision and is never modified.
func runLappi(ctx context.Context, in Input, taskID string, gaps []Gap) []Gap {
	if in.Lappi == nil || in.DiffEmpty || strings.TrimSpace(in.DiffContent) == "" {
		return nil
	}
	normalized := NormalizeGaps(append([]Gap(nil), gaps...))
	status, _ := StatusFromGaps(normalized, in.GateMode)
	blocking := 0
	for _, g := range normalized {
		if g.Blocking {
			blocking++
		}
	}
	outcome := in.Lappi.Run(ctx, in.DiffContent, lappi.AppChoice{Status: status, BlockingGaps: blocking})
	return lappiAdvisoryGaps(taskID, outcome.Decisions)
}

// lappiAdvisoryGaps turns model answers naming a defect into advisory gaps:
// at most one per file, and only for ModelAnswered with an answered,
// non-clean defect_class. Every other reading adds nothing.
func lappiAdvisoryGaps(taskID string, decisions []lappi.FileDecision) []Gap {
	var out []Gap
	seen := make(map[string]struct{}, len(decisions))
	for _, d := range decisions {
		if d.Result.Reading != lappi.ModelAnswered {
			continue
		}
		slot, ok := d.Result.Slots[lappi.DefectClassSlot]
		if !ok || slot.Noul || slot.Degraded {
			continue
		}
		value, ok := slot.Choice()
		if !ok || value == lappi.DefectClassClean {
			continue
		}
		if _, dup := seen[d.File.Path]; dup || d.File.Path == "" {
			continue
		}
		seen[d.File.Path] = struct{}{}
		path := d.File.Path
		out = append(out, Gap{
			ID:       StableGapID(taskID, "ADVISORY_LAPPI_DEFECT_CLASS", path),
			Severity: "low",
			GapType:  GapTypeLappiDefectClass,
			TaskID:   taskID,
			Description: "Lappi (advisory only; it blocks nothing and no gate checked it) read the change to " +
				path + " as " + value + ".",
			Evidence: []string{
				"lappi defect_class=" + value +
					" score=" + strconv.FormatFloat(slot.Score, 'f', 3, 64) +
					" backend=" + d.Result.Backend,
			},
			RecommendedFix: "Optional: review the change to " + path + " for a " + value +
				" defect. This advisory does not affect the verify status.",
			Blocking: false,
			File:     &path,
		})
	}
	return out
}

// isAdmissibleLappiGap is the admission rule as a predicate: a Lappi gap is
// advisory, non-blocking, and names no requirement or acceptance criterion.
func isAdmissibleLappiGap(g Gap) bool {
	return strings.HasPrefix(g.GapType, LappiAdvisoryPrefix) &&
		!g.Blocking &&
		g.RequirementID == nil &&
		g.AcceptanceCriterionID == nil
}

// appendLappiAdvisory appends the admissible Lappi gaps to gaps and nothing
// else; a gap that fails isAdmissibleLappiGap is dropped, never coerced.
func appendLappiAdvisory(gaps, advisory []Gap) []Gap {
	for _, g := range advisory {
		if isAdmissibleLappiGap(g) {
			gaps = append(gaps, g)
		}
	}
	return gaps
}
