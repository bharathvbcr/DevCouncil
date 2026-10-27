package verify

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/policy"
	"io/fs"
	"os"
	"os/exec"
	"strings"
	"syscall"
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
	meta.Env = in.EnvPolicy
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

	if in.GateOrigin == "invalid" {
		meta.GateOrigin, meta.GateError = in.GateOrigin, in.GateConfigError
		gaps = append(gaps, gateConfigGap(taskID, in))
	}
	if in.EnvPolicy.Err != "" {
		gaps = append(gaps, envConfigGap(taskID, in.EnvPolicy.Err))
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
	if in.Reviews.Recheck != nil && in.Reviews.SubjectErr == nil && in.Reviews.Subject != "" {
		after, err := in.Reviews.Recheck()
		switch {
		case err != nil:
			in.Reviews.SubjectErr = fmt.Errorf("the content could not be named again after the verification commands: %w", err)
		case after != in.Reviews.Subject:
			in.Reviews.SubjectErr = fmt.Errorf("the content changed while the verification commands ran "+
				"(%s before, %s after), so no review can be matched to what was verified", in.Reviews.Subject, after)
		}
	}
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
	gaps = append(gaps, dispatchCriteria(taskID, claimed, in.Requirements, in.RequirementsErr, commands, commandsPassed, in.Reviews)...)

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
	GateOrigin            string
	GateError             string
	Env                   EnvPolicy
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
		GateModeOrigin:        meta.GateOrigin,
		GateModeError:         meta.GateError,
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
		GateModeOrigin:        meta.GateOrigin,
		GateModeError:         meta.GateError,
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
//
// Path names are compared exactly as git wrote them. Every listing is
// NUL-delimited (-z), and no name is trimmed or has its separators rewritten:
// `f.txt ` and `f.txt` are two files, and folding them onto each other let one
// hide the other's bytes from the scan.
func CollectGitState(ctx context.Context, root string) (changed []string, diff string, empty bool, err error) {
	git, err := proc.NewGitSession(root)
	if err != nil {
		return nil, "", true, err
	}
	// One deadline for the whole collection, submodule walk included: the walk's
	// calls (DirtySubmodules, then each dirty submodule's files) draw on it
	// instead of each starting its own.
	ctx, cancel := proc.WithWalkBudget(ctx)
	defer cancel()
	nameOut, err := runGit(ctx, git, gitListingLimit, append([]string{"diff", "HEAD", "--name-status", "-z"}, toolStateExcludes()...)...)
	headless := false
	if err != nil {
		if !hasNoHead(ctx, git) {
			return nil, "", true, err
		}
		headless, nameOut = true, nil
	}
	entries, err := parseNameStatus(nameOut)
	if err != nil {
		return nil, "", true, err
	}
	seen := map[string]struct{}{}
	add := func(path string) {
		// Tool-owned state is not the agent's work; policy.ToolStateDirs
		// is the list the write policy protects, so the two cannot differ.
		if path == "" || policy.IsToolState(path) {
			return
		}
		if _, ok := seen[path]; !ok {
			changed = append(changed, path)
			seen[path] = struct{}{}
		}
	}
	rootFS, err := os.OpenRoot(root)
	if err != nil {
		return nil, "", true, err
	}
	defer func() { err = errors.Join(err, rootFS.Close()) }()
	// tracked is the set `git diff HEAD` carries a section for. A path git
	// reports as deleted but that exists in the working tree again (removed
	// from the index, then recreated) is not in it: its section is a deletion,
	// which carries none of the new bytes, so the diff is asked to leave it out
	// and the file is read from disk like any untracked one.
	tracked := map[string]struct{}{}
	var recreated []string
	for _, entry := range entries {
		if policy.IsToolState(entry.path) {
			continue
		}
		add(entry.path)
		if entry.deleted {
			info, serr := rootFS.Lstat(entry.path)
			switch {
			case errors.Is(serr, fs.ErrNotExist), errors.Is(serr, syscall.ENOTDIR):
				// Not there. ENOTDIR is "a parent of it is now a file": the
				// deleted file has no bytes either way, and the parent that
				// replaced its directory is read as the untracked file it is.
			case serr != nil:
				return nil, "", true, fmt.Errorf("cannot tell whether deleted %s is back on disk: %v", entry.path, serr)
			case !info.IsDir():
				recreated = append(recreated, entry.path)
				continue
			}
		}
		tracked[entry.path] = struct{}{}
	}
	untracked, err := runGit(ctx, git, gitListingLimit, "ls-files", "--others", "--exclude-standard", "-z")
	if err != nil {
		return nil, "", true, err
	}
	for _, name := range strings.Split(string(untracked), "\x00") {
		// A trailing slash is how git lists a repository nested in the tree that
		// is neither ignored nor a submodule: its files are in no listing.
		if strings.HasSuffix(name, "/") {
			return nil, "", true, proc.NestedRepositoryError(name)
		}
		add(name)
	}
	// The diff reads pass --ignore-submodules=dirty, so git does not run a
	// status inside each submodule; dirtiness comes from a guarded read of
	// each one, and a dirty submodule is a changed path as it was before.
	dirty, err := git.DirtySubmodules(ctx)
	if err != nil {
		return nil, "", true, err
	}
	for _, name := range dirty {
		add(name)
	}
	if !headless {
		args := append([]string{"-c", "core.quotePath=false", "diff", "HEAD"}, toolStateExcludes()...)
		for _, path := range recreated {
			args = append(args, ":(top,exclude,literal)"+path)
		}
		diffBytes, err := runGit(ctx, git, gitDiffLimit, args...)
		if err != nil {
			return nil, "", true, err
		}
		diff = string(diffBytes)
	}
	diff, err = attachMissingDiffBytes(ctx, git, root, changed, tracked, dirty, diff)
	if err != nil {
		return nil, "", true, err
	}
	empty = len(changed) == 0 && strings.TrimSpace(diff) == ""
	return changed, diff, empty, nil
}

// nameStatus is one path of `git diff --name-status -z`.
type nameStatus struct {
	path    string
	deleted bool
}

// parseNameStatus reads `git diff --name-status -z`: a status word, then one
// path (two for a rename or copy, whose destination is the path that changed),
// each NUL-terminated. A record that stops short is an error, never a shorter
// list.
func parseNameStatus(listing []byte) ([]nameStatus, error) {
	fields := strings.Split(string(listing), "\x00")
	if n := len(fields); n > 0 && fields[n-1] == "" {
		fields = fields[:n-1]
	}
	var out []nameStatus
	for i := 0; i < len(fields); {
		status := fields[i]
		i++
		if status == "" {
			return nil, errors.New("git diff --name-status named a change with no status")
		}
		paths := 1
		if status[0] == 'R' || status[0] == 'C' {
			paths = 2
		}
		if i+paths > len(fields) {
			return nil, fmt.Errorf("git diff --name-status ended inside a %q record", status)
		}
		path := fields[i+paths-1]
		i += paths
		if path == "" {
			return nil, fmt.Errorf("git diff --name-status named a %q change with no path", status)
		}
		out = append(out, nameStatus{path: path, deleted: status[0] == 'D'})
	}
	return out, nil
}

// ToolOwned reports whether a changed path is state a DevCouncil tool wrote,
// which verification never judges. See policy.ToolStateDirs.
func ToolOwned(path string) bool { return policy.IsToolState(path) }

// toolStateExcludes is the pathspec that leaves policy.ToolStateDirs out of a
// diff of the whole tree.
func toolStateExcludes() []string {
	spec := []string{"--", "."}
	for _, dir := range policy.ToolStateDirs {
		spec = append(spec, ":(top,exclude)"+dir)
	}
	return spec
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
	return runGitEnv(ctx, git, limit, 60*time.Second, nil, args...)
}

// runGitEnv is runGit with its own time bound and env added to git's
// environment (see proc.GitSession.RunEnv).
func runGitEnv(ctx context.Context, git *proc.GitSession, limit int, bound time.Duration, env []string, args ...string) ([]byte, error) {
	ctx, cancel := context.WithTimeout(ctx, bound)
	defer cancel()
	res, err := git.RunEnv(ctx, limit, env, args...)
	if err != nil {
		return nil, err
	}
	if res.Truncated {
		return nil, fmt.Errorf("git %s: %w (%d bytes kept)", strings.Join(args, " "), ErrGitOutputTruncated, limit)
	}
	return res.Stdout, nil
}

// persistBound bounds the store writes that record a run.
const persistBound = 30 * time.Second

// Persist writes the verification run and replaces gaps for the task.
//
// It runs detached from ctx's cancellation, under its own bound. A run that was
// cut short — the host's per-call budget, a cancelled request — is exactly the
// run whose record matters, and writing it on the expired ctx lost it.
//
// The run carries its evidence: each command with exit code, duration and an
// output tail, the run's real start and end, and the rigor account. Gaps are
// stored as the gate mode treats them (DemoteForMode), so a skipped run does
// not leave blocking rows behind for get_gaps to report.
func Persist(ctx context.Context, client *store.Client, taskID string, gaps []Gap, meta runMeta, status string) error {
	if client == nil {
		return nil
	}
	ctx, cancel := context.WithTimeout(context.WithoutCancel(ctx), persistBound)
	defer cancel()
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
		// Names only, never values: which variables the commands were allowed
		// beyond the base list, and which the repository asked for and was
		// refused.
		"env_scrub":            "allowlist",
		"env_allow_extra":      nonNil(meta.Env.Extra),
		"env_allow_refused":    nonNil(meta.Env.Refused),
		"env_allow_error":      meta.Env.Err,
		"gate_mode_origin":     meta.GateOrigin,
		"gate_mode_error":      meta.GateError,
		"rigor_applied":        rigorApplied,
		"rigor_skipped_reason": meta.RigorSkippedReason,
		"degraded_reasons":     meta.DegradedReasons,
		"gap_count":            len(stored),
		"blocking_gap_count":   blocking,
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
	// One transaction: the run is never stored with a status and no gaps, and
	// a failure leaves the task's previous gaps in place.
	if err := client.RecordVerification(ctx, &store.VerificationRun{
		ID:              runID,
		TaskID:          taskID,
		Sandbox:         meta.Sandbox,
		EnvironmentJSON: string(env),
		CommandsJSON:    string(cmds),
		Status:          status,
		StartedAt:       started.Format(time.RFC3339Nano),
		FinishedAt:      finished.Format(time.RFC3339Nano),
	}, taskID, toStoreGaps(stored)); err != nil {
		return fmt.Errorf("verification-record: %w", err)
	}
	return nil
}

func toStoreGaps(gaps []Gap) []store.GapRow {
	out := make([]store.GapRow, 0, len(gaps))
	for _, g := range gaps {
		evidence := g.Evidence
		if evidence == nil {
			evidence = []string{}
		}
		ev, _ := json.Marshal(evidence)
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

func nonNil(s []string) []string {
	if s == nil {
		return []string{}
	}
	return s
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
func VerifyTask(ctx context.Context, root string, client *store.Client, taskID string, gate GateState, sandbox, coveragePath string) (MCPResult, []Gap, error) {
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
	// The subject and the reviews are read only for a task that claims an
	// llm_review criterion: naming the working tree stages it into a scratch
	// index, which a task with nothing to review should not pay for.
	var reviews ReviewEvidence
	if linkedErr == nil {
		if ids := ReviewsNeeded(task.AcceptanceCriterionIDs, linked); len(ids) > 0 {
			reviews = LoadReviewEvidence(ctx, root, client, taskID, ids)
			reviews.Recheck = func() (string, error) { return WorkingTreeSubject(ctx, root) }
		}
	}
	envPolicy := LoadEnvPolicy(root)
	in := Input{
		Requirements:    linked,
		RequirementsErr: linkedErr,
		Reviews:         reviews,
		Root:            root,
		Task:            task,
		GateMode:        gate.Mode,
		GateOrigin:      gate.Origin,
		GateConfigPath:  gate.ConfigPath,
		GateConfigError: gate.Error,
		EnvPolicy:       envPolicy,
		Difficulty:      task.Difficulty,
		ChangedFiles:    changed,
		DiffContent:     diff,
		DiffEmpty:       empty,
		WorkPresent:     !empty,
		GitErr:          gitErr,
		AllowNoRigor:    allowNoRigor(),
		RunCommand:      RunCommandWithEnv(ctx, root, envPolicy),
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
	// A pass puts the task in review. It is a person who closes it to done: a
	// verifier proving the diff is not the owner accepting the work. Only a real
	// pass moves it — a skipped, degraded or blocked run does not — and only from
	// in progress, so verifying a task nobody checked out changes nothing.
	if result.Passed && result.Status == "verified" {
		// A verify run names its task by id; one with no item has no card to
		// move, which changes nothing, as for a task nobody checked out.
		if _, err := client.MoveTaskStatus(ctx, taskID, store.StatusInProgress, store.StatusReview); err != nil && !errors.Is(err, store.ErrNoSuchTask) {
			return result, gaps, fmt.Errorf("it passed, but the task could not be moved to review: %w", err)
		}
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
