package integrate

import (
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/devmap"
)

// A guide DevMap left alone because a person must act is reported as that,
// never as a write — and so is a disposition this host does not know. Both
// read as not current to a check. (The default branch once folded every
// unknown disposition into "wrote"; found in the adversarial round on
// 48a66ac3 when devmap began reporting malformed_block.)
func TestEveryGuideDispositionFoldsToWhatHappened(t *testing.T) {
	cases := map[string]map[Mode]string{
		"created":         {ModeApply: "wrote", ModeDryRun: "would_write"},
		"updated":         {ModeApply: "wrote", ModeDryRun: "would_write"},
		"adopted":         {ModeApply: "wrote", ModeDryRun: "would_write"},
		"unchanged":       {ModeApply: "unchanged", ModeDryRun: "unchanged"},
		"not_ours":        {ModeApply: "not_ours", ModeDryRun: "not_ours"},
		"malformed_block": {ModeApply: "needs_attention", ModeDryRun: "needs_attention"},
		"refused":         {ModeApply: "needs_attention", ModeDryRun: "needs_attention"},
		"from_the_future": {ModeApply: "unknown:from_the_future", ModeDryRun: "unknown:from_the_future"},
	}
	for disposition, want := range cases {
		for mode, state := range want {
			receipt := &Receipt{Files: map[string]string{}}
			foldReport(receipt, "/repo", mode, &devmap.IntegrateReport{
				Guides: []devmap.Guide{{Path: "/repo/CLAUDE.md", Disposition: disposition}},
			})
			if got := receipt.Files["CLAUDE.md"]; got != state {
				t.Errorf("%s in %v: %q, want %q", disposition, mode, got, state)
			}
			current := len(receipt.Drifted()) == 0
			if wantCurrent := state == "unchanged" || state == "not_ours"; current != wantCurrent {
				t.Errorf("%s in %v: current=%v, want %v", disposition, mode, current, wantCurrent)
			}
		}
	}
}
