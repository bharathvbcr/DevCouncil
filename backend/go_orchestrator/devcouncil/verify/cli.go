package verify

import (
	"context"
	"fmt"
	"strings"
	"unicode"
	"unicode/utf8"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/console"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
)

// RunCLI implements
// `devcouncil verify [TASK_ID] [--json] [--sandbox local] [--coverage PATH]`.
//
// A sandbox other than local is refused with exit 2; see ParseSandbox.
//
// coveragePath is empty unless the operator names a profile. The rigor gates
// that read added lines run regardless; only the diff∩coverage gate needs one,
// and without it the report says so rather than reporting a diff as exercised.
func RunCLI(ctx context.Context, root string, client *store.Client, taskID string, gate GateState, sandbox, coveragePath string, jsonOut bool) int {
	if gate.Mode == "" {
		gate.Mode = "off"
	}
	cli := CLIResult{
		OK:             true,
		GateMode:       gate.Mode,
		GateModeOrigin: gateOriginIfInvalid(gate),
		GateModeError:  gateErrorIfInvalid(gate),
		Tasks:          []TaskCLIResult{},
	}
	if client == nil {
		return reportNotRun(cli, NotRunStoreUnavailable, taskID,
			"DevCouncil state is unavailable in this directory.", jsonOut)
	}
	sandbox, err := ParseSandbox(sandbox)
	if err != nil {
		// A usage error, like an unknown flag: nothing ran, nothing was
		// recorded, and the operator asked for something this host cannot do.
		writeJSON(map[string]any{"ok": false, "error": err.Error()})
		return 2
	}
	var taskIDs []string
	if taskID != "" {
		taskIDs = []string{taskID}
	} else {
		ready, err := client.ReadyTasks(ctx, store.ReadyMode(gate.Mode))
		if err != nil {
			writeJSON(map[string]any{"ok": false, "error": err.Error()})
			return 1
		}
		// ReadyTasks may be empty; fall back to listing is not available —
		// require an explicit task id when nothing is ready.
		for _, t := range ready {
			taskIDs = append(taskIDs, t.ID)
		}
		if len(taskIDs) == 0 {
			writeJSON(map[string]any{"ok": false, "error": "No tasks found to verify."})
			return 1
		}
	}

	for _, id := range taskIDs {
		mcp, gaps, err := VerifyTask(ctx, root, client, id, gate, sandbox, coveragePath)
		if err != nil {
			if reason, notRun := notRunReason(err); notRun {
				return reportNotRun(cli, reason, id, err.Error(), jsonOut)
			}
			cli.OK = false
			cli.Error = err.Error()
			writeJSON(cli)
			return 1
		}
		meta := runMeta{
			Sandbox:               mcp.Sandbox,
			GateMode:              mcp.GateMode,
			GateOrigin:            mcp.GateModeOrigin,
			GateError:             mcp.GateModeError,
			Difficulty:            mcp.Difficulty,
			DiffEmpty:             mcp.DiffEmpty,
			CoverageMeasured:      mcp.CoverageMeasured,
			CoverageSkippedReason: mcp.CoverageSkippedReason,
			CompilerActive:        mcp.CompilerActive,
			VerificationMode:      mcp.VerificationMode,
			RigorApplied:          mcp.RigorApplied,
			// Carried across with RigorApplied, never separately. A CLI entry
			// rebuilt with the gate list but not the reason reports an empty
			// rigor_applied with nothing to explain it — which is the reading
			// the pair exists to make impossible, reintroduced one surface
			// down from where it was fixed.
			RigorSkippedReason: mcp.RigorSkippedReason,
		}
		entry := ToCLITask(id, gaps, meta)
		// The verdict VerifyTask earned, which knows the run-level degraded
		// reasons that meta (rebuilt from the payload) does not carry.
		entry.Status, entry.DegradedReasons = mcp.Status, mcp.DegradedReasons
		cli.Tasks = append(cli.Tasks, entry)
		cli.ProcessedTasks++
		cli.TotalGaps += entry.GapCount
		if entry.Status == "blocked" {
			cli.BlockedTasks++
			cli.OK = false
		} else if entry.Status == "degraded" {
			cli.DegradedTasks++
			cli.OK = false
		} else if entry.VerificationSkipped {
			cli.CompletedWithoutVerification++
		} else {
			cli.VerifiedTasks++
		}
	}
	if jsonOut {
		if !writeJSON(cli) {
			return 1
		}
	} else {
		printHuman(cli)
	}
	if !cli.OK {
		return 1
	}
	return 0
}

// printHuman is the human-mode verdict summary: the totals, then one line per
// task that was judged or not run.
func printHuman(cli CLIResult) {
	console.Printf("verified=%d skipped=%d blocked=%d degraded=%d gaps=%d\n", cli.VerifiedTasks, cli.CompletedWithoutVerification, cli.BlockedTasks, cli.DegradedTasks, cli.TotalGaps)
	for _, t := range cli.Tasks {
		console.Printf("  %s status=%s gaps=%d blocking=%d\n", humanText(t.TaskID), t.Status, t.GapCount, t.BlockingGapCount)
	}
}

// reportNotRun ends a verify that stopped without judging the task it was on.
//
// When nothing was judged before it (ProcessedTasks == 0) the whole result is a
// not-run: top-level not_run, and in human mode one stderr line. When earlier
// tasks were judged their verdicts are kept (tasks[] and the human summary) and
// the failed task is reported as its own tasks[] entry with status "not_run"
// and a not_run object, and top-level not_run stays absent: not_run is absent
// on any run that judged something. Either way ok is false and the exit is 1.
func reportNotRun(cli CLIResult, reason, taskID, message string, jsonOut bool) int {
	cli.OK = false
	cli.Error = message
	nr := &NotRun{Reason: reason, TaskID: taskID}
	if cli.ProcessedTasks == 0 {
		cli.NotRun = nr
	} else {
		cli.Tasks = append(cli.Tasks, TaskCLIResult{
			TaskID: taskID, Status: "not_run", NotRun: nr, GateMode: cli.GateMode,
			GateModeOrigin: cli.GateModeOrigin, GateModeError: cli.GateModeError,
			RigorApplied: []string{}, Gaps: []Gap{}, NextActions: []NextAction{}, AdvisoryActions: []NextAction{},
		})
	}
	if jsonOut {
		writeJSON(cli)
		return 1
	}
	if cli.ProcessedTasks > 0 {
		printHuman(cli)
	}
	if reason == NotRunTaskNotFound {
		message = fmt.Sprintf("no task %q in this repository's task store", taskID)
	} else {
		message = humanText(message)
	}
	console.Errorf("verify: NOT RUN (%s): %s\n", reason, message)
	return 1
}

// humanText makes store-supplied text safe for a terminal: control characters
// (C0, C1, DEL), bidi and other non-printing runes are written as escapes, so
// a store error or a task id cannot move the cursor or rewrite the line.
func humanText(s string) string {
	var b strings.Builder
	for _, r := range s {
		switch {
		case r == utf8.RuneError, r != ' ' && !unicode.IsPrint(r):
			if r > 0xffff {
				fmt.Fprintf(&b, "\\U%08x", r)
			} else if r > 0xff || r == utf8.RuneError {
				fmt.Fprintf(&b, "\\u%04x", r)
			} else {
				fmt.Fprintf(&b, "\\x%02x", r)
			}
		default:
			b.WriteRune(r)
		}
	}
	return b.String()
}

func writeJSON(v any) bool {
	if err := console.JSON(v); err != nil {
		console.Errorf("verify output: %v\n", err)
		return false
	}
	return true
}

// gateOriginIfInvalid is the origin only when it says the config was bad; a
// healthy origin adds nothing to the envelope.
func gateOriginIfInvalid(g GateState) string {
	if g.Origin == "invalid" {
		return g.Origin
	}
	return ""
}

func gateErrorIfInvalid(g GateState) string {
	if g.Origin == "invalid" {
		return g.Error
	}
	return ""
}
