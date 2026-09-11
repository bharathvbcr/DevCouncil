package verify

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
)

// AllowedNextToolsForVerify mirrors Python integrations/mcp/util.allowed_next_tools
// for a planned/in-progress task. Duplicated here to avoid an import cycle with
// package devcouncil (which calls into verify).
func AllowedNextToolsForVerify() []string {
	return []string{
		"devcouncil_read_file",
		"devcouncil_get_evidence",
		"devcouncil_get_diff",
		"devcouncil_run_command",
		"devcouncil_apply_patch",
		"devcouncil_write_file",
		"devcouncil_update_task_scope",
		"devcouncil_verify_task",
	}
}

// Run executes the Phase-5 verification gates for one task and returns gaps.
//
// Coverage that cannot run is recorded as CoverageSkippedReason — never as a
// measured pass. Commands that cannot run become skipped/invalid gaps.
func Run(in Input) (gaps []Gap, meta runMeta) {
	meta.Sandbox = in.Sandbox
	if meta.Sandbox == "" {
		meta.Sandbox = "local"
	}
	meta.GateMode = in.GateMode
	if meta.GateMode == "" {
		meta.GateMode = "enforce"
	}
	meta.Difficulty = in.Difficulty
	if meta.Difficulty == "" && in.Task != nil {
		meta.Difficulty = in.Task.Difficulty
	}
	if meta.Difficulty == "" {
		meta.Difficulty = "easy"
	}
	meta.DiffEmpty = in.DiffEmpty
	meta.RigorApplied = []string{}

	taskID := ""
	var planned []dc.PlannedFile
	if in.Task != nil {
		taskID = in.Task.ID
		planned = in.Task.PlannedFiles
	}

	workPresent := in.WorkPresent || (!in.DiffEmpty && len(in.ChangedFiles) > 0)
	if g := DetectNoWorkGap(taskID, planned, workPresent); g != nil {
		gaps = append(gaps, *g)
	}
	gaps = append(gaps, DetectPlannedFileGaps(taskID, planned, in.ChangedFiles)...)
	gaps = append(gaps, DetectOrphanDiffGaps(taskID, planned, in.ChangedFiles, nil)...)
	gaps = append(gaps, DetectDependencyRiskGaps(taskID, planned, in.ChangedFiles)...)

	commands := in.Commands
	if len(commands) == 0 && in.Task != nil {
		commands = in.Task.ExpectedTests
		if len(commands) == 0 {
			commands = in.Task.AllowedCommands
		}
	}
	gaps = append(gaps, RunVerificationCommands(taskID, commands, in.RunCommand)...)

	// Diff∩coverage: when there is no diff, skip with an explicit reason.
	// Never report coverage_measured=true / pass for "could not measure".
	if in.DiffEmpty || strings.TrimSpace(in.DiffContent) == "" {
		meta.CoverageMeasured = false
		meta.CoverageSkippedReason = "no diff to measure"
	} else {
		meta.CoverageMeasured = false
		meta.CoverageSkippedReason = "coverage profile not supplied"
	}

	meta.CompilerActive = false
	meta.VerificationMode = "coarse"
	gaps = NormalizeGaps(gaps)
	return gaps, meta
}

type runMeta struct {
	Sandbox               string
	GateMode              string
	Difficulty            string
	DiffEmpty             bool
	CoverageMeasured      bool
	CoverageSkippedReason string
	CompilerActive        bool
	VerificationMode      string
	RigorApplied          []string
}

// StatusFromGaps maps gaps to blocked/verified under gate_mode.
func StatusFromGaps(gaps []Gap, gateMode string) (status string, passed bool) {
	blocking := 0
	for _, g := range gaps {
		if g.Blocking {
			blocking++
		}
	}
	if gateMode == "off" {
		return "verified", true
	}
	if blocking > 0 {
		return "blocked", false
	}
	return "verified", true
}

// ToMCP builds the leased verify_task payload.
func ToMCP(taskID string, gaps []Gap, meta runMeta) MCPResult {
	status, passed := StatusFromGaps(gaps, meta.GateMode)
	blocking, advisory := SplitNextActions(gaps)
	blockingGaps := make([]Gap, 0)
	for _, g := range gaps {
		if g.Blocking {
			blockingGaps = append(blockingGaps, g)
		}
	}
	if blockingGaps == nil {
		blockingGaps = []Gap{}
	}
	rigor := meta.RigorApplied
	if rigor == nil {
		rigor = []string{}
	}
	return MCPResult{
		OK:                    true,
		TaskID:                taskID,
		Passed:                passed,
		Status:                status,
		GateMode:              meta.GateMode,
		Difficulty:            meta.Difficulty,
		DiffEmpty:             meta.DiffEmpty,
		CoverageMeasured:      meta.CoverageMeasured,
		CoverageSkippedReason: meta.CoverageSkippedReason,
		CompilerActive:        meta.CompilerActive,
		VerificationMode:      meta.VerificationMode,
		VerificationSkipped:   false,
		Sandbox:               meta.Sandbox,
		RigorApplied:          rigor,
		BlockingGaps:          blockingGaps,
		NextActions:           blocking,
		AdvisoryActions:       advisory,
		AllowedNextTools:      AllowedNextToolsForVerify(),
	}
}

// ToCLITask builds one CLI task entry.
func ToCLITask(taskID string, gaps []Gap, meta runMeta) TaskCLIResult {
	status, _ := StatusFromGaps(gaps, meta.GateMode)
	blocking, advisory := SplitNextActions(gaps)
	blockingCount := 0
	for _, g := range gaps {
		if g.Blocking {
			blockingCount++
		}
	}
	rigor := meta.RigorApplied
	if rigor == nil {
		rigor = []string{}
	}
	if gaps == nil {
		gaps = []Gap{}
	}
	return TaskCLIResult{
		TaskID:                taskID,
		Status:                status,
		GateMode:              meta.GateMode,
		Difficulty:            meta.Difficulty,
		DiffEmpty:             meta.DiffEmpty,
		CoverageMeasured:      meta.CoverageMeasured,
		CoverageSkippedReason: meta.CoverageSkippedReason,
		CompilerActive:        meta.CompilerActive,
		VerificationMode:      meta.VerificationMode,
		VerificationSkipped:   false,
		RigorApplied:          rigor,
		GapCount:              len(gaps),
		BlockingGapCount:      blockingCount,
		Gaps:                  gaps,
		NextActions:           blocking,
		AdvisoryActions:       advisory,
	}
}

// CollectGitState probes the working tree for changed files and unified diff.
func CollectGitState(ctx context.Context, root string) (changed []string, diff string, empty bool, err error) {
	nameOut, err := runGit(ctx, root, "diff", "HEAD", "--name-only", "-z")
	if err != nil {
		// Fall back to unstaged + untracked listing when HEAD is missing.
		nameOut, err = runGit(ctx, root, "ls-files", "--others", "--exclude-standard", "-z")
		if err != nil {
			return nil, "", true, err
		}
	}
	for _, p := range strings.Split(string(nameOut), "\x00") {
		p = strings.TrimSpace(p)
		if p != "" {
			changed = append(changed, strings.ReplaceAll(p, "\\", "/"))
		}
	}
	// Also include untracked.
	untracked, _ := runGit(ctx, root, "ls-files", "--others", "--exclude-standard", "-z")
	seen := make(map[string]struct{}, len(changed))
	for _, c := range changed {
		seen[c] = struct{}{}
	}
	for _, p := range strings.Split(string(untracked), "\x00") {
		p = strings.TrimSpace(strings.ReplaceAll(p, "\\", "/"))
		if p == "" {
			continue
		}
		if _, ok := seen[p]; !ok {
			changed = append(changed, p)
			seen[p] = struct{}{}
		}
	}
	diffBytes, _ := runGit(ctx, root, "diff", "HEAD")
	diff = string(diffBytes)
	empty = len(changed) == 0 && strings.TrimSpace(diff) == ""
	return changed, diff, empty, nil
}

func runGit(ctx context.Context, root string, args ...string) ([]byte, error) {
	ctx, cancel := context.WithTimeout(ctx, 60*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, "git", args...)
	cmd.Dir = root
	var stdout, stderr bytes.Buffer
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr
	// Match the module-wide subprocess contract: group isolation so a timeout
	// reaches grandchildren, and RunBounded so a wedged Start cannot outlive
	// the caller's deadline. See proc.ConfigureGroup / proc.RunBounded.
	proc.ConfigureGroup(cmd)
	cmd.WaitDelay = 2 * time.Second
	err, timedOut := proc.RunBounded(ctx, cmd.Run)
	if timedOut {
		return nil, fmt.Errorf("git %v did not return within the deadline: %w", args, ctx.Err())
	}
	if err != nil {
		if len(stdout.Bytes()) > 0 {
			return stdout.Bytes(), err
		}
		return nil, err
	}
	_ = stderr
	return stdout.Bytes(), nil
}

// Persist writes the verification run and replaces gaps for the task.
func Persist(ctx context.Context, client *store.Client, taskID string, gaps []Gap, meta runMeta, status string) error {
	if client == nil {
		return nil
	}
	now := time.Now().UTC().Format(time.RFC3339Nano)
	runID := "run-" + taskID + "-" + shortHash(now+status)
	env, _ := json.Marshal(map[string]any{
		"gate_mode":               meta.GateMode,
		"difficulty":              meta.Difficulty,
		"diff_empty":              meta.DiffEmpty,
		"coverage_measured":       meta.CoverageMeasured,
		"coverage_skipped_reason": meta.CoverageSkippedReason,
		"verification_mode":       meta.VerificationMode,
	})
	cmds, _ := json.Marshal([]string{})
	if err := client.RunRecord(ctx, store.VerificationRun{
		ID:              runID,
		TaskID:          taskID,
		Sandbox:         meta.Sandbox,
		EnvironmentJSON: string(env),
		CommandsJSON:    string(cmds),
		Status:          status,
		StartedAt:       now,
		FinishedAt:      now,
	}); err != nil {
		return fmt.Errorf("run-record: %w", err)
	}
	if err := client.GapsReplace(ctx, taskID, toStoreGaps(gaps)); err != nil {
		return fmt.Errorf("gaps-replace: %w", err)
	}
	return nil
}

func toStoreGaps(gaps []Gap) []store.GapRow {
	out := make([]store.GapRow, 0, len(gaps))
	for _, g := range gaps {
		ev, _ := json.Marshal(g.Evidence)
		out = append(out, store.GapRow{
			ID:             g.ID,
			Severity:       g.Severity,
			GapType:        g.GapType,
			TaskID:         g.TaskID,
			Description:    g.Description,
			RecommendedFix: g.RecommendedFix,
			Blocking:       g.Blocking,
			EvidenceJSON:   ev,
		})
	}
	return out
}

func shortHash(s string) string {
	sum := sha256.Sum256([]byte(s))
	return hex.EncodeToString(sum[:])[:10]
}

// VerifyTask is the high-level entry used by MCP and CLI.
func VerifyTask(ctx context.Context, root string, client *store.Client, taskID, gateMode, sandbox string) (MCPResult, []Gap, error) {
	if client == nil {
		return MCPResult{}, nil, fmt.Errorf("store unavailable")
	}
	task, err := client.Task(ctx, taskID)
	if err != nil {
		return MCPResult{}, nil, err
	}
	if task == nil {
		return MCPResult{}, nil, fmt.Errorf("task %s not found", taskID)
	}
	changed, diff, empty, err := CollectGitState(ctx, root)
	if err != nil {
		// Not a git repo / git missing: treat as empty work with an explicit skip,
		// never as a pass.
		changed, diff, empty = nil, "", true
	}
	in := Input{
		Root:         root,
		Task:         task,
		GateMode:     gateMode,
		Sandbox:      sandbox,
		Difficulty:   task.Difficulty,
		ChangedFiles: changed,
		DiffContent:  diff,
		DiffEmpty:    empty,
		WorkPresent:  !empty,
		RunCommand:   DefaultRunCommand(root),
	}
	gaps, meta := Run(in)
	result := ToMCP(taskID, gaps, meta)
	_ = Persist(ctx, client, taskID, gaps, meta, result.Status)
	return result, gaps, nil
}
