package verify

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"os"
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

// SandboxLocal is the only isolation verify provides: none. Verification
// commands run on the host, through /bin/sh -c in the project root
// (DefaultRunCommand).
const SandboxLocal = "local"

// ParseSandbox resolves a requested sandbox to the one verify will actually
// run in, or refuses it.
//
// `docker` and `nix` used to be accepted here, ignored, and written onto the
// report and the store as the sandbox the run used — so a run that executed on
// the host could be recorded as isolated (TASK-P7-2). A request for isolation
// this host does not have is refused rather than downgraded: an operator who
// asked for a container is relying on it, and running their commands on the
// host anyway is exactly the outcome they asked to avoid.
func ParseSandbox(requested string) (string, error) {
	switch strings.TrimSpace(requested) {
	case "", SandboxLocal:
		return SandboxLocal, nil
	default:
		return "", fmt.Errorf(
			"sandbox %q is not implemented: verification commands run on the host "+
				"(/bin/sh -c in the project root), so the only sandbox is %q",
			requested, SandboxLocal)
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
	// What ran, not what was asked for. Input has no sandbox field: the
	// request is validated by ParseSandbox at the entry points, and the only
	// value that survives it is the one this function executes in.
	meta.Sandbox = SandboxLocal
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
	if in.GitErr != nil {
		gaps = append(gaps, gitStateGap(taskID, in.GitErr))
	}

	commands := in.Commands
	if len(commands) == 0 && in.Task != nil {
		commands = in.Task.ExpectedTests
		if len(commands) == 0 {
			commands = in.Task.AllowedCommands
		}
	}
	commandGaps, commandRecords := runVerificationCommands(taskID, commands, in.RunCommand)
	gaps = append(gaps, commandGaps...)
	meta.Commands = commandRecords

	// Each claimed acceptance criterion, dispatched on its verification
	// method. Commands count as passing only if at least one ran and none
	// produced a gap: no commands is no evidence, not clean evidence.
	var claimed []string
	if in.Task != nil {
		claimed = in.Task.AcceptanceCriterionIDs
	}
	commandsPassed := len(commands) > 0 && len(commandGaps) == 0
	gaps = append(gaps, dispatchCriteria(taskID, claimed, in.Requirements, in.RequirementsErr, commands, commandsPassed)...)

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
	meta.DegradedReasons = append(meta.DegradedReasons, rigor.degradedReasons...)
	if rigor.unavailable && !in.AllowNoRigor && modeEnforces(in.GateMode) {
		// The secret and stub scans never ran. In enforce that is not a pass;
		// the operator opts out explicitly, or installs dcverify.
		meta.DegradedReasons = append(meta.DegradedReasons, rigor.skippedReason)
	}

	// Lappi, admission only: it may append advisory, non-blocking gaps and
	// nothing else (verify/lappi.go). Nil in.Lappi — the default — skips it.
	gaps = appendLappiAdvisory(gaps, runLappi(ctx, in, taskID, gaps))

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
	// DegradedReasons are checks that could not run and did not become a
	// blocking gap. A run with any is "degraded": never passed.
	DegradedReasons []string
	// Commands is what each verification command did, for the stored run.
	Commands   []CommandRecord
	StartedAt  time.Time
	FinishedAt time.Time
}

// gitStateGap reports that the git reads verification judges failed. Blocking
// and hard-safety (gating.IsHardSafetyGap): an advisory mode must not turn
// "git could not be read" into a pass on a diff nobody looked at.
func gitStateGap(taskID string, err error) Gap {
	return Gap{
		ID:       StableGapID(taskID, "GITSTATE"),
		Severity: "high",
		GapType:  "rigor_check_unavailable",
		TaskID:   taskID,
		Description: "The working tree's changes could not be read from git, so the diff-based gates " +
			"(scope, stub detection, secret scanning, coverage) did not examine this change: " + err.Error(),
		Evidence:       []string{"git: " + err.Error()},
		RecommendedFix: "Make git readable in the project root (is it a repository, is git installed and on PATH?), then re-verify.",
		Blocking:       true,
	}
}

func modeEnforces(gateMode string) bool {
	return strings.TrimSpace(strings.ToLower(gateMode)) == "enforce"
}

func modeAdvisory(gateMode string) bool {
	mode := strings.TrimSpace(strings.ToLower(gateMode))
	return mode == "advisory" || mode == "warn"
}

// DemoteForMode returns gaps as the gate mode treats them: under advisory the
// demotable ones lose Blocking, and under a skipped mode none block. Every
// surface that lists gaps (blocking_gaps, next_actions, advisory_actions, the
// stored rows) reads this, so the verdict and what it lists cannot disagree —
// StatusFromGaps used to demote for the verdict only while the lists still
// said blocking:true. The input is not modified.
func DemoteForMode(gaps []Gap, gateMode string) []Gap {
	skipped := verificationSkipped(gateMode)
	advisory := modeAdvisory(gateMode)
	out := make([]Gap, len(gaps))
	copy(out, gaps)
	for i := range out {
		if !out[i].Blocking {
			continue
		}
		if skipped || (advisory && gating.MayDemote(out[i].GapType, true)) {
			out[i].Blocking = false
		}
	}
	return out
}

// Verdict is the status a run earns: StatusFromGaps, then "degraded" when the
// run is clean only because something did not run or its failure was demoted.
//
// Degraded is a distinct outcome and never passed: a failing expected command
// demoted by advisory mode is a failing command, and an enforce run whose
// secret and stub scans never executed has not examined the diff. Manvi
// already treats degraded as never-passed.
func Verdict(gaps []Gap, meta runMeta) (status string, passed bool, reasons []string) {
	status, passed = StatusFromGaps(gaps, meta.GateMode)
	if status != "verified" {
		return status, passed, nil
	}
	reasons = append(reasons, meta.DegradedReasons...)
	if modeAdvisory(meta.GateMode) {
		for _, g := range gaps {
			if g.Blocking && g.GapType == "test_failed" {
				reasons = append(reasons, "expected verification command failed (demoted by advisory mode): "+g.Description)
			}
		}
	}
	if len(reasons) > 0 {
		return "degraded", false, reasons
	}
	return status, passed, nil
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
	status, passed, reasons := Verdict(gaps, meta)
	skipped := verificationSkipped(meta.GateMode)
	gaps = DemoteForMode(gaps, meta.GateMode)
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
		DegradedReasons:       reasons,
		BlockingGaps:          blockingGaps,
		NextActions:           blocking,
		AdvisoryActions:       advisory,
		AllowedNextTools:      AllowedNextToolsForVerify(),
	}
}

// ToCLITask builds one CLI task entry.
func ToCLITask(taskID string, gaps []Gap, meta runMeta) TaskCLIResult {
	status, _, reasons := Verdict(gaps, meta)
	skipped := verificationSkipped(meta.GateMode)
	gaps = DemoteForMode(gaps, meta.GateMode)
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
		DegradedReasons:       reasons,
		GapCount:              len(gaps),
		BlockingGapCount:      blockingCount,
		Gaps:                  gaps,
		NextActions:           blocking,
		AdvisoryActions:       advisory,
	}
}

// CollectGitState probes the working tree for changed files and unified diff.
//
// Every read goes through one proc.GitSession, so the repository's config is
// read once for the operation. A read that fails is reported, never read as
// "nothing changed": the one failure that has a meaning of its own is a
// repository with no HEAD yet, which lists its untracked files instead. A
// missing object in a partial clone — git no longer fetches one on demand —
// is an error like any other.
func CollectGitState(ctx context.Context, root string) (changed []string, diff string, empty bool, err error) {
	git, err := proc.NewGitSession(root)
	if err != nil {
		return nil, "", true, err
	}
	nameOut, err := runGit(ctx, git, gitListingLimit, "diff", "HEAD", "--name-only", "-z")
	headless := false
	if err != nil {
		if !hasNoHead(ctx, git) {
			return nil, "", true, err
		}
		headless, nameOut = true, nil
	}
	seen := map[string]struct{}{}
	add := func(listing []byte) {
		for _, p := range strings.Split(string(listing), "\x00") {
			p = strings.TrimSpace(strings.ReplaceAll(p, "\\", "/"))
			if p == "" {
				continue
			}
			if _, ok := seen[p]; !ok {
				changed = append(changed, p)
				seen[p] = struct{}{}
			}
		}
	}
	add(nameOut)
	untracked, err := runGit(ctx, git, gitListingLimit, "ls-files", "--others", "--exclude-standard", "-z")
	if err != nil {
		return nil, "", true, err
	}
	add(untracked)
	// The diff reads pass --ignore-submodules=dirty, so git does not run a
	// status inside each submodule; dirtiness comes from a guarded read of
	// each one, and a dirty submodule is a changed path as it was before.
	dirty, err := git.DirtySubmodules(ctx)
	if err != nil {
		return nil, "", true, err
	}
	add([]byte(strings.Join(dirty, "\x00")))
	if !headless {
		diffBytes, err := runGit(ctx, git, gitDiffLimit, "-c", "core.quotePath=false", "diff", "HEAD")
		if err != nil {
			return nil, "", true, err
		}
		diff = string(diffBytes)
	}
	empty = len(changed) == 0 && strings.TrimSpace(diff) == ""
	return changed, diff, empty, nil
}

// hasNoHead reports whether the repository has no HEAD commit yet: git ran
// and answered that HEAD does not resolve. A git that could not run, or was
// refused, is not that answer.
func hasNoHead(ctx context.Context, git *proc.GitSession) bool {
	_, err := runGit(ctx, git, 1<<10, "rev-parse", "--verify", "--quiet", "HEAD")
	var exit *exec.ExitError
	return errors.As(err, &exit)
}

// Output caps for the git reads verification judges. The diff is what the
// stub and secret scanners read, so a diff that does not fit is refused rather
// than scored on its first bytes: a scan of part of a change must never read
// as a scan of the change.
const (
	gitDiffLimit    = 32 << 20
	gitListingLimit = 8 << 20
)

// ErrGitOutputTruncated is returned when a git read exceeded its output cap.
var ErrGitOutputTruncated = errors.New("git output exceeded the verification cap")

// runGit runs a git read through the shared proc.GitSession runner. The diff
// verification judges comes from git, and a git the repository supplies — or
// a program its .git/config names — could report any diff it likes,
// including none.
func runGit(ctx context.Context, git *proc.GitSession, limit int, args ...string) ([]byte, error) {
	ctx, cancel := context.WithTimeout(ctx, 60*time.Second)
	defer cancel()
	res, err := git.Run(ctx, limit, args...)
	if err != nil {
		return nil, err
	}
	if res.Truncated {
		return nil, fmt.Errorf("git %s: %w (%d bytes kept)", strings.Join(args, " "), ErrGitOutputTruncated, limit)
	}
	return res.Stdout, nil
}

// Persist writes the verification run and replaces gaps for the task.
//
// The run carries its evidence: each command with exit code, duration and an
// output tail, the run's real start and end, and the rigor account. Gaps are
// stored as the gate mode treats them (DemoteForMode), so a skipped run does
// not leave blocking rows behind for get_gaps to report.
func Persist(ctx context.Context, client *store.Client, taskID string, gaps []Gap, meta runMeta, status string) error {
	if client == nil {
		return nil
	}
	started, finished := meta.StartedAt, meta.FinishedAt
	if started.IsZero() {
		started = time.Now().UTC()
	}
	if finished.Before(started) {
		finished = started
	}
	stored := DemoteForMode(gaps, meta.GateMode)
	blocking := 0
	for _, g := range stored {
		if g.Blocking {
			blocking++
		}
	}
	runID := "run-" + taskID + "-" + shortHash(started.Format(time.RFC3339Nano)+status)
	rigorApplied := meta.RigorApplied
	if rigorApplied == nil {
		rigorApplied = []string{}
	}
	env, err := json.Marshal(map[string]any{
		"gate_mode":               meta.GateMode,
		"difficulty":              meta.Difficulty,
		"diff_empty":              meta.DiffEmpty,
		"coverage_measured":       meta.CoverageMeasured,
		"coverage_skipped_reason": meta.CoverageSkippedReason,
		"verification_mode":       meta.VerificationMode,
		"rigor_applied":           rigorApplied,
		"rigor_skipped_reason":    meta.RigorSkippedReason,
		"degraded_reasons":        meta.DegradedReasons,
		"gap_count":               len(stored),
		"blocking_gap_count":      blocking,
	})
	if err != nil {
		return fmt.Errorf("run-record: %w", err)
	}
	records := meta.Commands
	if records == nil {
		records = []CommandRecord{}
	}
	cmds, err := json.Marshal(records)
	if err != nil {
		return fmt.Errorf("run-record: %w", err)
	}
	if err := client.RunRecord(ctx, store.VerificationRun{
		ID:              runID,
		TaskID:          taskID,
		Sandbox:         meta.Sandbox,
		EnvironmentJSON: string(env),
		CommandsJSON:    string(cmds),
		Status:          status,
		StartedAt:       started.Format(time.RFC3339Nano),
		FinishedAt:      finished.Format(time.RFC3339Nano),
	}); err != nil {
		return fmt.Errorf("run-record: %w", err)
	}
	if err := client.GapsReplace(ctx, taskID, toStoreGaps(stored)); err != nil {
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

			RequirementID:              g.RequirementID,
			AcceptanceCriterionID:      g.AcceptanceCriterionID,
			ExpectedVerificationMethod: g.ExpectedVerificationMethod,
			File:                       g.File,
			Line:                       g.Line,
			SuggestedCommand:           g.SuggestedCommand,
			StdoutPath:                 g.StdoutPath,
			StderrPath:                 g.StderrPath,
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
//
// sandbox is refused unless ParseSandbox accepts it, before anything runs or
// is persisted.
func VerifyTask(ctx context.Context, root string, client *store.Client, taskID, gateMode, sandbox, coveragePath string) (MCPResult, []Gap, error) {
	if _, err := ParseSandbox(sandbox); err != nil {
		return MCPResult{}, nil, err
	}
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
	changed, diff, empty, gitErr := CollectGitState(ctx, root)
	if gitErr != nil {
		// Not a git repo, git missing or refused, or a read past its cap: the
		// diff-based gates have nothing to judge, and Run reports that as a
		// blocking rigor_check_unavailable gap rather than judging an empty
		// diff as a clean one.
		changed, diff, empty = nil, "", true
	}
	// Read only when the task claims criteria, so a task without any costs no
	// extra store round trip. A read error is carried, not returned: it
	// becomes a blocking gap per criterion, beside everything else this run
	// can still say.
	var linked store.LinkedRequirements
	var linkedErr error
	if len(task.AcceptanceCriterionIDs) > 0 {
		linked, linkedErr = client.Requirements(ctx, taskID)
	}
	in := Input{
		Requirements:    linked,
		RequirementsErr: linkedErr,
		Root:            root,
		Task:            task,
		GateMode:        gateMode,
		Difficulty:      task.Difficulty,
		ChangedFiles:    changed,
		DiffContent:     diff,
		DiffEmpty:       empty,
		WorkPresent:     !empty,
		GitErr:          gitErr,
		AllowNoRigor:    allowNoRigor(),
		RunCommand:      DefaultRunCommand(ctx, root),
		Rigor:           RigorClient(root),
		CoveragePath:    coveragePath,
		Lappi:           LappiAsker(),
	}
	began := time.Now().UTC()
	gaps, meta := Run(ctx, in)
	meta.StartedAt, meta.FinishedAt = began, time.Now().UTC()
	result := ToMCP(taskID, gaps, meta)
	persistErr := Persist(ctx, client, taskID, gaps, meta, result.Status)
	writeBlockedCorrection(root, taskID, &result, gaps)
	if persistErr != nil {
		// The verdict is returned with the error so a caller can show it, but
		// as an error: the gates and the task loop read the store, and a run
		// the store never recorded must not be acted on as if it had been.
		return result, gaps, fmt.Errorf("its result was not recorded in the store: %w", persistErr)
	}
	return result, gaps, nil
}

// AllowNoRigorEnv is the explicit opt-out for an enforce run without dcverify:
// set to 1, true or yes, a run whose secret and stub scans cannot run is not
// degraded for that alone. It exists for repositories that have chosen to
// verify without the rigor component; the report still says rigor was skipped.
const AllowNoRigorEnv = "DEVCOUNCIL_ALLOW_NO_DCVERIFY"

func allowNoRigor() bool {
	switch strings.ToLower(strings.TrimSpace(os.Getenv(AllowNoRigorEnv))) {
	case "1", "true", "yes":
		return true
	}
	return false
}

// writeBlockedCorrection persists a repair brief when a check ran and
// blocked. A write failure must not hide the verify result — gaps are already
// in the store. A skipped or passing verify writes nothing.
func writeBlockedCorrection(root, taskID string, result *MCPResult, gaps []Gap) {
	if result == nil || result.Passed || result.VerificationSkipped || result.Status == "degraded" {
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
