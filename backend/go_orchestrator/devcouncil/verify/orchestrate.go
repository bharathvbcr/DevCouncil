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
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/correction"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/gating"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
)

// AllowedNextToolsForVerify names the tools an agent may call after a verify.
//
// These are the eight this host actually serves — the set devcouncil.Registry
// advertises through Specs() and tools/list. The list is spelled here rather
// than read from the registry because package devcouncil imports verify, so
// reading it back would be an import cycle. devcouncil's
// allowed_next_tools_test.go holds the two lists to each other in both
// directions, which is the seam a second copy needs to be safe.
//
// It previously carried the Python surface's names — devcouncil_read_file,
// devcouncil_write_file, devcouncil_apply_patch, devcouncil_run_command,
// devcouncil_get_evidence, devcouncil_update_task_scope — of which six of eight
// are served by nothing here (GAP-P7-NEXT-TOOLS-DRIFT). An agent that followed
// them called into a tool the host does not have and got an error in place of
// the repair step the verifier had just told it to take.
func AllowedNextToolsForVerify() []string {
	return []string{
		"devcouncil_get_diff",
		"devcouncil_checkout_task",
		"devcouncil_renew_lease",
		"devcouncil_release_task",
		"devcouncil_next_task",
		"devcouncil_verify_task",
		"devcouncil_get_gaps",
		"devcouncil_policy_check_write",
	}
}

// Run executes the Phase-5 verification gates for one task and returns gaps.
//
// Coverage that cannot run is recorded as CoverageSkippedReason — never as a
// measured pass. Commands that cannot run become skipped/invalid gaps. The
// same rule governs the rigor gates: RigorApplied names what ran, and
// RigorSkippedReason says why when nothing did.
//
// ctx bounds the subprocess work — the rigor gates are a dcverify child — so a
// cancelled verify does not leave one running.
func Run(ctx context.Context, in Input) (gaps []Gap, meta runMeta) {
	meta.Sandbox = in.Sandbox
	if meta.Sandbox == "" {
		meta.Sandbox = "local"
	}
	meta.GateMode = in.GateMode
	if meta.GateMode == "" {
		meta.GateMode = "off"
	}
	meta.Difficulty = in.Difficulty
	if meta.Difficulty == "" && in.Task != nil {
		meta.Difficulty = in.Task.Difficulty
	}
	if meta.Difficulty == "" {
		meta.Difficulty = "easy"
	}
	meta.DiffEmpty = in.DiffEmpty

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

	// Stub detection, secret scanning and diff∩coverage, all of which live in
	// dcverify. The outcome carries its own account of what ran: never
	// coverage_measured=true for "could not measure", and never an empty
	// RigorApplied that a reader could take for "no findings".
	rigor := runRigorGates(ctx, in, taskID, plannedExpectingChange(planned))
	gaps = append(gaps, rigor.gaps...)
	meta.RigorApplied = rigor.applied
	meta.RigorSkippedReason = rigor.skippedReason
	meta.CoverageMeasured = rigor.coverageMeasured
	meta.CoverageSkippedReason = rigor.coverageSkippedReason

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
	// RigorApplied names the rigor gates that ran; RigorSkippedReason says why
	// when none did. Exactly one is populated — see verify/rigor.go, where an
	// empty RigorApplied with no reason beside it is the ambiguity this pair
	// was introduced to remove.
	RigorApplied       []string
	RigorSkippedReason string
}

// StatusFromGaps maps gaps to blocked/verified under gate_mode.
//
// off skips quality verification. advisory still blocks hard-safety gaps.
// enforce blocks every gap marked Blocking. Unknown spellings cannot
// silently enforce — they skip, matching gatescfg.Normalize.
func StatusFromGaps(gaps []Gap, gateMode string) (status string, passed bool) {
	if verificationSkipped(gateMode) {
		return "skipped", false
	}
	mode := strings.TrimSpace(strings.ToLower(gateMode))
	advisory := mode == "advisory" || mode == "warn"
	for _, g := range gaps {
		if !g.Blocking {
			continue
		}
		if advisory && gating.MayDemote(g.GapType, true) {
			continue
		}
		return "blocked", false
	}
	return "verified", true
}

func verificationSkipped(gateMode string) bool {
	mode := strings.TrimSpace(strings.ToLower(gateMode))
	switch mode {
	case "advisory", "warn", "enforce", "true", "1", "yes":
		return false
	default:
		return true
	}
}

// ToMCP builds the leased verify_task payload.
func ToMCP(taskID string, gaps []Gap, meta runMeta) MCPResult {
	status, passed := StatusFromGaps(gaps, meta.GateMode)
	skipped := verificationSkipped(meta.GateMode)
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
		VerificationSkipped:   skipped,
		Sandbox:               meta.Sandbox,
		RigorApplied:          rigor,
		RigorSkippedReason:    meta.RigorSkippedReason,
		BlockingGaps:          blockingGaps,
		NextActions:           blocking,
		AdvisoryActions:       advisory,
		AllowedNextTools:      AllowedNextToolsForVerify(),
	}
}

// ToCLITask builds one CLI task entry.
func ToCLITask(taskID string, gaps []Gap, meta runMeta) TaskCLIResult {
	status, _ := StatusFromGaps(gaps, meta.GateMode)
	skipped := verificationSkipped(meta.GateMode)
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
		VerificationSkipped:   skipped,
		RigorApplied:          rigor,
		RigorSkippedReason:    meta.RigorSkippedReason,
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
//
// coveragePath is an optional coverage profile for the same change; empty means
// the diff∩coverage gate does not run, which is reported as
// CoverageSkippedReason rather than as a measured pass. MCP passes empty — its
// tool schema has no field for a path, and inventing one would be a protocol
// change — so today that gate is reachable from `devcouncil verify --coverage`
// and from a library caller that runs its own tests under coverage.
func VerifyTask(ctx context.Context, root string, client *store.Client, taskID, gateMode, sandbox, coveragePath string) (MCPResult, []Gap, error) {
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
		Rigor:        RigorClient(root),
		CoveragePath: coveragePath,
	}
	gaps, meta := Run(ctx, in)
	result := ToMCP(taskID, gaps, meta)
	_ = Persist(ctx, client, taskID, gaps, meta, result.Status)
	writeBlockedCorrection(root, taskID, &result, gaps)
	return result, gaps, nil
}

// writeBlockedCorrection persists a repair brief when a check ran and
// blocked. A write failure must not hide the verify result — gaps are already
// in the store. A skipped or passing verify writes nothing.
func writeBlockedCorrection(root, taskID string, result *MCPResult, gaps []Gap) {
	if result == nil || result.Passed || result.VerificationSkipped {
		return
	}
	if root == "" || taskID == "" {
		return
	}
	manifestGaps := make([]correction.ManifestGap, 0, len(gaps))
	for _, g := range gaps {
		if !g.Blocking {
			continue
		}
		file := ""
		if g.File != nil {
			file = *g.File
		}
		manifestGaps = append(manifestGaps, correction.ManifestGap{
			ID: g.ID, GapType: g.GapType, Severity: g.Severity,
			Blocking: true, Action: g.RecommendedFix, File: file,
		})
	}
	actions := make([]string, 0, len(result.NextActions))
	for _, a := range result.NextActions {
		actions = append(actions, a.Action)
	}
	_, path, err := correction.Write(correction.WriteOptions{
		Root: root, TaskID: taskID, Gaps: manifestGaps, NextActions: actions,
	})
	if err != nil {
		return
	}
	result.CorrectionPath = path
}
