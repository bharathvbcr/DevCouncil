package verify

import (
	"context"
	"encoding/json"
	"fmt"
	"os"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
)

// RunCLI implements `devcouncil verify [TASK_ID] [--json] [--sandbox local]`.
func RunCLI(ctx context.Context, root string, client *store.Client, taskID, gateMode, sandbox string, jsonOut bool) int {
	if client == nil {
		payload := map[string]any{"ok": false, "error": "DevCouncil state is unavailable in this directory."}
		writeJSON(payload)
		return 1
	}
	if sandbox == "" {
		sandbox = "local"
	}
	if gateMode == "" {
		gateMode = "enforce"
	}

	var taskIDs []string
	if taskID != "" {
		taskIDs = []string{taskID}
	} else {
		ids, err := client.ReadyTasks(ctx)
		if err != nil {
			writeJSON(map[string]any{"ok": false, "error": err.Error()})
			return 1
		}
		// ReadyTasks may be empty; fall back to listing is not available —
		// require an explicit task id when nothing is ready.
		taskIDs = ids
		if len(taskIDs) == 0 {
			writeJSON(map[string]any{"ok": false, "error": "No tasks found to verify."})
			return 1
		}
	}

	cli := CLIResult{
		OK:                           true,
		GateMode:                     gateMode,
		ProcessedTasks:               0,
		VerifiedTasks:                0,
		BlockedTasks:                 0,
		CompletedWithoutVerification: 0,
		TotalGaps:                    0,
		Tasks:                        []TaskCLIResult{},
	}
	for _, id := range taskIDs {
		mcp, gaps, err := VerifyTask(ctx, root, client, id, gateMode, sandbox)
		if err != nil {
			cli.OK = false
			cli.Error = err.Error()
			writeJSON(cli)
			return 1
		}
		meta := runMeta{
			Sandbox:               mcp.Sandbox,
			GateMode:              mcp.GateMode,
			Difficulty:            mcp.Difficulty,
			DiffEmpty:             mcp.DiffEmpty,
			CoverageMeasured:      mcp.CoverageMeasured,
			CoverageSkippedReason: mcp.CoverageSkippedReason,
			CompilerActive:        mcp.CompilerActive,
			VerificationMode:      mcp.VerificationMode,
			RigorApplied:          mcp.RigorApplied,
		}
		entry := ToCLITask(id, gaps, meta)
		cli.Tasks = append(cli.Tasks, entry)
		cli.ProcessedTasks++
		cli.TotalGaps += entry.GapCount
		if entry.Status == "blocked" {
			cli.BlockedTasks++
			cli.OK = false
		} else {
			cli.VerifiedTasks++
		}
	}
	if jsonOut {
		writeJSON(cli)
	} else {
		fmt.Printf("verified=%d blocked=%d gaps=%d\n", cli.VerifiedTasks, cli.BlockedTasks, cli.TotalGaps)
		for _, t := range cli.Tasks {
			fmt.Printf("  %s status=%s gaps=%d blocking=%d\n", t.TaskID, t.Status, t.GapCount, t.BlockingGapCount)
		}
	}
	if !cli.OK {
		return 1
	}
	return 0
}

func writeJSON(v any) {
	enc := json.NewEncoder(os.Stdout)
	enc.SetIndent("", "  ")
	_ = enc.Encode(v)
}
