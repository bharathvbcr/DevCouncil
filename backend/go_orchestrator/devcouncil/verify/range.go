package verify

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"sort"
	"strings"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/dcverify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/gating"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/gitrev"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/policy"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
)

// RangeState is what one collection of a commit range (`baseSHA..headSHA`)
// found. Unlike CollectGitStateReport (which inspects a local working tree and
// excludes tool-state directories from task scope), CollectRangeState inspects
// an immutable git commit range for pull-request verification:
//
//   - Every read goes through a guarded proc.GitSession (fsmonitor, hooksPath,
//     replace refs, ext-diff, textconv, color, and filter drivers neutralized).
//   - Sensitive configuration/workflow paths (`.gitattributes`,
//     `.github/workflows/*`, `action.yml`, `.devcouncil/*`, `.devmap/*`) are
//     always kept in Changed, scanned for secrets, and flagged in SensitiveFiles.
//   - Text paths whose diff git suppressed as binary (e.g. `.gitattributes -diff`
//     or `binary`) are recorded in BinarySuppressed (degrading the run) AND
//     re-diffed with `--text` so hidden secrets/stubs are still scanned.
//   - A diff exceeding gitDiffLimit sets DiffTooLarge and clears Diff rather
//     than running a partial scan.
type RangeState struct {
	BaseSHA string
	HeadSHA string

	Changed []string
	Added   map[string]struct{}
	Deleted map[string]struct{}

	Diff         string
	Empty        bool
	DiffTooLarge bool

	MovedGitlinks    []proc.GitlinkChange
	BinarySuppressed []string
	BinaryPaths      []string
	SensitiveFiles   []string
	DependencyFiles  []string
}

// RangeDegradedCode identifies why a range verification run was degraded.
// Fixed vocabulary so downstream renderers (PR comments, SARIF, annotations)
// never leak raw stderr or PR source text.
const (
	DegradedDiffTooLarge            = "diff_too_large"
	DegradedGitlinkMoved            = "gitlink_moved"
	DegradedBinaryDiffSuppressed    = "binary_diff_suppressed"
	DegradedSensitivePathChanged    = "sensitive_path_changed"
	DegradedStubASTFallback         = "stub_ast_degraded"
	DegradedBinarySecretScanSkipped = "binary_secret_scan_skipped"
)

// RangeDegraded is one structured degradation observed during range verification.
type RangeDegraded struct {
	Code   string `json:"code"`
	Path   string `json:"path,omitempty"`
	Detail string `json:"detail,omitempty"`
}

// RangeErrorCode identifies a fail-closed error in RunRange.
const (
	RangeErrInvalidMode         = "invalid_mode"
	RangeErrGitError            = "git_error"
	RangeErrDirtyWorktree       = "dirty_worktree"
	RangeErrDCVerifyMissing     = "dcverify_missing"
	RangeErrDCVerifyFailed      = "dcverify_failed"
	RangeErrRequiredGatesAbsent = "required_gates_missing"
	RangeErrEmptyDiffWithPaths  = "empty_diff_with_changes"
)

// ErrDirtyTrackedWorktree is returned by CollectRangeState when tracked files
// in the working tree have uncommitted modifications. dcverify reads binary
// post-images from disk, so a dirty tracked worktree cannot be verified safely
// against an immutable commit range.
var ErrDirtyTrackedWorktree = errors.New("tracked working tree has uncommitted changes")

// RangeInput is everything RunRange needs. GateMode is explicit and NEVER read
// from the untrusted PR tree's `.devcouncil/config.yaml`. AllowNoRigor does not
// exist here: PR gate fails closed when dcverify is missing or fails.
type RangeInput struct {
	Root         string
	BaseSHA      string
	HeadSHA      string
	GateMode     string
	State        RangeState
	GitErr       error
	Rigor        *dcverify.Client
	CoveragePath string
}

// RangeResult is the deterministic outcome of verifying a commit range.
type RangeResult struct {
	Status   string
	Passed   bool
	GateMode string
	BaseSHA  string
	HeadSHA  string

	DiffEmpty        bool
	ChangedFiles     []string
	SensitiveFiles   []string
	DependencyFiles  []string
	BinarySuppressed []string
	BinaryPaths      []string
	MovedGitlinks    []proc.GitlinkChange

	Gaps             []Gap
	BlockingGapCount int
	HardSafetyCount  int
	StubAllowedCount int
	StubAllowed      []Gap

	RigorApplied         []string
	RigorError           bool
	RigorErrorCode       string
	RigorErrorDetail     string
	StubASTNotApplicable bool

	CoverageMeasured      bool
	CoverageSkippedReason string

	Degraded        []RangeDegraded
	DegradedReasons []string
}

// IsSensitivePRPath reports whether path controls gate policy, git diff
// attributes, submodule definitions, CI execution, or DevMap/DevCouncil
// configuration. Changes to any sensitive path are always listed and raise the
// PR gate to at least degraded.
func IsSensitivePRPath(path string) bool {
	norm := strings.ToLower(strings.ReplaceAll(path, "\\", "/"))
	norm = strings.TrimPrefix(norm, "./")
	if norm == "" {
		return false
	}
	if policy.IsToolState(norm) {
		return true
	}
	if strings.HasPrefix(norm, ".github/workflows/") || strings.HasPrefix(norm, ".github/actions/") {
		return true
	}
	base := norm
	if i := strings.LastIndexByte(norm, '/'); i >= 0 {
		base = norm[i+1:]
	}
	switch base {
	case ".gitattributes", ".gitmodules", ".devmapignore", ".devcouncilignore", "action.yml", "action.yaml":
		return true
	}
	return false
}

// IsDependencyManifestPath reports whether path is a dependency manifest or
// lockfile (matching depFileNames in checks.go).
func IsDependencyManifestPath(path string) bool {
	norm := strings.ReplaceAll(path, "\\", "/")
	base := norm
	if i := strings.LastIndexByte(norm, '/'); i >= 0 {
		base = norm[i+1:]
	}
	_, ok := depFileNames[base]
	return ok
}

// CollectRangeState collects changed files, gitlinks, binary-suppression
// signals, sensitive/dependency paths, and the unified diff for baseSHA..headSHA.
func CollectRangeState(ctx context.Context, root, baseSHA, headSHA string) (RangeState, error) {
	if !gitrev.IsFullSHA(baseSHA) {
		return RangeState{}, fmt.Errorf("base revision %q is not a full commit SHA", baseSHA)
	}
	if !gitrev.IsFullSHA(headSHA) {
		return RangeState{}, fmt.Errorf("head revision %q is not a full commit SHA", headSHA)
	}
	git, err := proc.NewGitSession(root)
	if err != nil {
		return RangeState{}, err
	}
	ctx, cancel := proc.WithWalkBudget(ctx)
	defer cancel()

	// Require a clean tracked working tree before collecting range state:
	// dcverify reads post-image bytes for binary diff sections from the working
	// tree, and devmap indexes the working tree. Untracked files (such as
	// devmap's local index) are excluded via --untracked-files=no.
	statusOut, err := runGit(ctx, git, gitListingLimit, "status", "--porcelain", "--untracked-files=no")
	if err != nil {
		return RangeState{}, err
	}
	if len(bytes.TrimSpace(statusOut)) > 0 {
		return RangeState{}, ErrDirtyTrackedWorktree
	}

	state := RangeState{
		BaseSHA: baseSHA,
		HeadSHA: headSHA,
		Added:   map[string]struct{}{},
		Deleted: map[string]struct{}{},
	}

	nameOut, err := runGit(ctx, git, gitListingLimit, "diff", "--name-status", "-z", "--no-renames", baseSHA, headSHA)
	if err != nil {
		return RangeState{}, err
	}
	entries, err := parseNameStatus(nameOut)
	if err != nil {
		return RangeState{}, err
	}
	seen := make(map[string]struct{}, len(entries))
	for _, e := range entries {
		if e.path == "" {
			continue
		}
		if _, dup := seen[e.path]; !dup {
			seen[e.path] = struct{}{}
			state.Changed = append(state.Changed, e.path)
		}
		if e.added {
			state.Added[e.path] = struct{}{}
		}
		if e.deleted {
			state.Deleted[e.path] = struct{}{}
		}
	}
	sort.Strings(state.Changed)

	for _, p := range state.Changed {
		if IsSensitivePRPath(p) {
			state.SensitiveFiles = append(state.SensitiveFiles, p)
		} else if IsDependencyManifestPath(p) {
			state.DependencyFiles = append(state.DependencyFiles, p)
		}
	}

	moved, err := rangeMovedGitlinks(ctx, git, baseSHA, headSHA)
	if err != nil {
		return RangeState{}, err
	}
	state.MovedGitlinks = moved
	gitlinkSet := make(map[string]struct{}, len(moved))
	for _, g := range moved {
		gitlinkSet[g.Path] = struct{}{}
	}

	if len(state.Changed) == 0 {
		state.Empty = true
		return state, nil
	}

	// Detect binary sections from --numstat (`-\t-\t<path>`). Any non-deleted,
	// non-gitlink path whose blob at headSHA contains no NUL byte in its first
	// binaryProbeBytes was suppressed by `.gitattributes` (`-diff`/`binary`).
	numstatOut, err := runGit(ctx, git, gitListingLimit, "diff", "--numstat", "-z", "--no-renames", baseSHA, headSHA)
	if err != nil {
		return RangeState{}, err
	}
	for _, record := range bytes.Split(numstatOut, []byte{0}) {
		if len(record) == 0 {
			continue
		}
		parts := strings.SplitN(string(record), "\t", 3)
		if len(parts) != 3 || parts[2] == "" {
			return RangeState{}, errors.New("git diff --numstat returned an unreadable record")
		}
		if parts[0] != "-" || parts[1] != "-" {
			continue
		}
		p := parts[2]
		if _, isDel := state.Deleted[p]; isDel {
			continue
		}
		if _, isGL := gitlinkSet[p]; isGL {
			continue
		}
		isBin, probeErr := probeBlobBinary(ctx, git, headSHA, p)
		if probeErr != nil {
			return RangeState{}, probeErr
		}
		if isBin {
			state.BinaryPaths = append(state.BinaryPaths, p)
		} else {
			state.BinarySuppressed = append(state.BinarySuppressed, p)
		}
	}
	sort.Strings(state.BinarySuppressed)
	sort.Strings(state.BinaryPaths)

	// Collect unified diff. Exclude text paths suppressed by attributes from
	// the primary pass and re-diff them with `--text` so each path appears at
	// most once in the combined diff (dcverify rejects duplicate path sections)
	// while still scanning lines hidden behind `.gitattributes -diff`.
	primaryArgs := []string{"-c", "core.quotePath=false", "diff", "--no-renames", baseSHA, headSHA}
	if len(state.BinarySuppressed) > 0 {
		primaryArgs = append(primaryArgs, "--", ".")
		for _, p := range state.BinarySuppressed {
			primaryArgs = append(primaryArgs, ":(top,exclude,literal)"+p)
		}
	}
	diffBytes, err := runGit(ctx, git, gitDiffLimit, primaryArgs...)
	if errors.Is(err, ErrGitOutputTruncated) {
		state.DiffTooLarge = true
		state.Diff = ""
		return state, nil
	}
	if err != nil {
		return RangeState{}, err
	}

	var combined strings.Builder
	combined.Write(diffBytes)

	if len(state.BinarySuppressed) > 0 {
		remaining := gitDiffLimit - combined.Len()
		if remaining <= 0 {
			state.DiffTooLarge = true
			state.Diff = ""
			return state, nil
		}
		textArgs := append([]string{"-c", "core.quotePath=false", "diff", "--text", "--no-renames", baseSHA, headSHA}, literalSpec(state.BinarySuppressed, false)...)
		textBytes, err := runGit(ctx, git, remaining, textArgs...)
		if errors.Is(err, ErrGitOutputTruncated) {
			state.DiffTooLarge = true
			state.Diff = ""
			return state, nil
		}
		if err != nil {
			return RangeState{}, err
		}
		if combined.Len() > 0 && len(textBytes) > 0 && !strings.HasSuffix(combined.String(), "\n") {
			combined.WriteByte('\n')
		}
		combined.Write(textBytes)
		if combined.Len() > gitDiffLimit {
			state.DiffTooLarge = true
			state.Diff = ""
			return state, nil
		}
	}

	state.Diff = combined.String()
	state.Empty = len(state.Changed) == 0 && strings.TrimSpace(state.Diff) == ""
	return state, nil
}

// probeBlobBinary inspects the first binaryProbeBytes of headSHA:path for a NUL byte.
func probeBlobBinary(ctx context.Context, git *proc.GitSession, headSHA, path string) (bool, error) {
	readCtx, cancel := context.WithTimeout(ctx, gitReadBound)
	defer cancel()
	res, err := git.Run(readCtx, binaryProbeBytes, "cat-file", "blob", headSHA+":"+path)
	if err != nil {
		return false, fmt.Errorf("inspecting blob %s at %s: %w", path, headSHA, err)
	}
	return bytes.IndexByte(res.Stdout, 0) >= 0, nil
}

// rangeMovedGitlinks parses `git diff --raw -z --no-abbrev --no-renames base head`
// for records where either side is a gitlink (mode 160000).
func rangeMovedGitlinks(ctx context.Context, git *proc.GitSession, baseSHA, headSHA string) ([]proc.GitlinkChange, error) {
	rawOut, err := runGit(ctx, git, gitListingLimit, "diff", "--raw", "-z", "--no-abbrev", "--no-renames", baseSHA, headSHA)
	if err != nil {
		return nil, err
	}
	if len(rawOut) == 0 {
		return nil, nil
	}
	fields := strings.Split(string(rawOut), "\x00")
	if n := len(fields); n > 0 && fields[n-1] == "" {
		fields = fields[:n-1]
	}
	const gitlinkMode = "160000"
	const zeroMode = "000000"
	var moved []proc.GitlinkChange
	for i := 0; i < len(fields); {
		hdr := fields[i]
		i++
		if !strings.HasPrefix(hdr, ":") {
			return nil, fmt.Errorf("git diff --raw returned unexpected record %q", hdr)
		}
		parts := strings.Fields(hdr[1:])
		if len(parts) < 5 {
			return nil, fmt.Errorf("git diff --raw returned short header %q", hdr)
		}
		oldMode, newMode, status := parts[0], parts[1], parts[4]
		needPaths := 1
		if len(status) > 0 && (status[0] == 'R' || status[0] == 'C') {
			needPaths = 2
		}
		if i+needPaths > len(fields) {
			return nil, fmt.Errorf("git diff --raw ended inside record for status %q", status)
		}
		p := fields[i+needPaths-1]
		i += needPaths
		if oldMode != gitlinkMode && newMode != gitlinkMode {
			continue
		}
		var shape string
		switch {
		case oldMode == gitlinkMode && newMode == gitlinkMode:
			shape = proc.GitlinkMoved
		case oldMode == zeroMode:
			shape = proc.GitlinkAdded
		case newMode == zeroMode:
			shape = proc.GitlinkDeleted
		case oldMode == gitlinkMode:
			shape = proc.GitlinkSubmoduleToFile
		default:
			shape = proc.GitlinkFileToSubmodule
		}
		moved = append(moved, proc.GitlinkChange{Path: p, Shape: shape})
	}
	sort.Slice(moved, func(i, j int) bool {
		if moved[i].Path != moved[j].Path {
			return moved[i].Path < moved[j].Path
		}
		return moved[i].Shape < moved[j].Shape
	})
	return moved, nil
}

// NormalizeRangeMode validates and normalizes an explicit PR gate mode.
// Empty defaults to "enforce". Unknown spellings are refused (fail closed).
func NormalizeRangeMode(mode string) (string, error) {
	m := strings.TrimSpace(strings.ToLower(mode))
	switch m {
	case "", "enforce":
		return "enforce", nil
	case "advisory", "warn":
		return "advisory", nil
	case "off":
		return "off", nil
	default:
		return "", fmt.Errorf("invalid gate mode %q: expected enforce, advisory, or off", mode)
	}
}

// RunRange runs static diff verification over a commit range without task
// semantics (no orphan_diff, no task-scoped dependency_risk blocks, no shell
// verification commands, no AllowNoRigor).
func RunRange(ctx context.Context, in RangeInput) RangeResult {
	mode, err := NormalizeRangeMode(in.GateMode)
	if err != nil {
		return RangeResult{
			Status:           "error",
			Passed:           false,
			GateMode:         in.GateMode,
			BaseSHA:          in.BaseSHA,
			HeadSHA:          in.HeadSHA,
			RigorError:       true,
			RigorErrorCode:   RangeErrInvalidMode,
			RigorErrorDetail: err.Error(),
			Gaps:             []Gap{},
			RigorApplied:     []string{},
			Degraded:         []RangeDegraded{},
		}
	}

	res := RangeResult{
		GateMode:         mode,
		BaseSHA:          in.BaseSHA,
		HeadSHA:          in.HeadSHA,
		DiffEmpty:        in.State.Empty,
		ChangedFiles:     copySorted(in.State.Changed),
		SensitiveFiles:   copySorted(in.State.SensitiveFiles),
		DependencyFiles:  copySorted(in.State.DependencyFiles),
		BinarySuppressed: copySorted(in.State.BinarySuppressed),
		BinaryPaths:      copySorted(in.State.BinaryPaths),
		MovedGitlinks:    append([]proc.GitlinkChange(nil), in.State.MovedGitlinks...),
		Gaps:             []Gap{},
		StubAllowed:      []Gap{},
		RigorApplied:     []string{},
		Degraded:         []RangeDegraded{},
	}

	if mode == "off" {
		res.Status = "skipped"
		res.Passed = false
		return res
	}

	const taskID = "PR"

	if in.GitErr != nil {
		g := gitStateGap(taskID, in.GitErr)
		res.Status = "error"
		res.Passed = false
		res.RigorError = true
		if errors.Is(in.GitErr, ErrDirtyTrackedWorktree) {
			res.RigorErrorCode = RangeErrDirtyWorktree
		} else {
			res.RigorErrorCode = RangeErrGitError
		}
		res.RigorErrorDetail = in.GitErr.Error()
		res.Gaps = NormalizeGaps([]Gap{g})
		res.BlockingGapCount = 1
		res.HardSafetyCount = 1
		return res
	}

	var gaps []Gap
	var meta runMeta
	meta.GateMode = mode
	meta.DiffEmpty = in.State.Empty

	// Record structural range degradations first (these hold regardless of
	// dcverify findings).
	if in.State.DiffTooLarge {
		res.Degraded = append(res.Degraded, RangeDegraded{Code: DegradedDiffTooLarge})
		meta.DegradedReasons = append(meta.DegradedReasons, diffTooLargeError{}.Error())
	}
	for _, gl := range in.State.MovedGitlinks {
		res.Degraded = append(res.Degraded, RangeDegraded{
			Code:   DegradedGitlinkMoved,
			Path:   gl.Path,
			Detail: gl.Shape,
		})
	}
	meta.DegradedReasons = append(meta.DegradedReasons, gitlinkReasons(in.State.MovedGitlinks)...)
	for _, p := range in.State.BinarySuppressed {
		res.Degraded = append(res.Degraded, RangeDegraded{
			Code: DegradedBinaryDiffSuppressed,
			Path: p,
		})
		meta.DegradedReasons = append(meta.DegradedReasons, "diff suppressed as binary on text path: "+toolStatePath(p))
	}
	for _, p := range in.State.SensitiveFiles {
		res.Degraded = append(res.Degraded, RangeDegraded{
			Code: DegradedSensitivePathChanged,
			Path: p,
		})
		meta.DegradedReasons = append(meta.DegradedReasons, "sensitive configuration or workflow path changed: "+toolStatePath(p))
	}

	// Never skip dcverify silently in PR gate: missing dcverify is always an error.
	if in.Rigor == nil {
		res.RigorError = true
		res.RigorErrorCode = RangeErrDCVerifyMissing
		res.RigorErrorDetail = "dcverify binary not configured"
		gaps = append(gaps, Gap{
			ID:             StableGapID(taskID, "RIGOR"),
			Severity:       "high",
			GapType:        "rigor_check_unavailable",
			TaskID:         taskID,
			Description:    "dcverify binary is not configured; PR gate requires secret_scan and stub_detection.",
			Evidence:       []string{"dcverify: missing"},
			RecommendedFix: "Install dcverify and pass --dcverify or ensure it is on PATH outside the repository.",
			Blocking:       true,
		})
	} else if !in.State.DiffTooLarge {
		// Pass BinaryPaths as planned so dcverify reads and secret-scans the
		// post-image bytes of binary sections from the clean working tree
		// (same pattern as addToolStateSecretScan's ToolStateBinaryPaths).
		// Cap at maxToolStateBinaryPaths and degrade any overflow so no binary
		// path is silently skipped.
		binaryPlanned := copySorted(in.State.BinaryPaths)
		if len(binaryPlanned) > maxToolStateBinaryPaths {
			for _, extra := range binaryPlanned[maxToolStateBinaryPaths:] {
				res.Degraded = append(res.Degraded, RangeDegraded{
					Code: DegradedBinarySecretScanSkipped,
					Path: extra,
				})
				meta.DegradedReasons = append(meta.DegradedReasons, "secret scan did not read binary file past path cap: "+toolStatePath(extra))
			}
			binaryPlanned = binaryPlanned[:maxToolStateBinaryPaths]
		}
		inner := Input{
			Root:         in.Root,
			GateMode:     mode,
			ChangedFiles: in.State.Changed,
			AddedFiles:   in.State.Added,
			DiffContent:  in.State.Diff,
			DiffEmpty:    in.State.Empty,
			Rigor:        in.Rigor,
			CoveragePath: in.CoveragePath,
		}
		rigor := runDiffRigorGates(ctx, inner, taskID, binaryPlanned)
		gaps = append(gaps, rigor.gaps...)
		res.RigorApplied = append([]string(nil), rigor.applied...)
		res.CoverageMeasured = rigor.coverageMeasured
		res.CoverageSkippedReason = rigor.coverageSkippedReason
		meta.RigorApplied = rigor.applied
		meta.RigorSkippedReason = rigor.skippedReason
		meta.CoverageMeasured = rigor.coverageMeasured
		meta.CoverageSkippedReason = rigor.coverageSkippedReason
		meta.DegradedReasons = append(meta.DegradedReasons, rigor.degradedReasons...)

		for _, reason := range rigor.degradedReasons {
			switch {
			case strings.HasPrefix(reason, "secret scan did not read "):
				res.Degraded = append(res.Degraded, RangeDegraded{Code: DegradedBinarySecretScanSkipped})
			default:
				res.Degraded = append(res.Degraded, RangeDegraded{Code: DegradedStubASTFallback})
			}
		}

		// Detect dcverify errors or empty diff with changed paths.
		for _, g := range rigor.gaps {
			if g.GapType == "rigor_check_unavailable" {
				res.RigorError = true
				if strings.TrimSpace(in.State.Diff) == "" && len(in.State.Changed) > 0 {
					res.RigorErrorCode = RangeErrEmptyDiffWithPaths
				} else {
					res.RigorErrorCode = RangeErrDCVerifyFailed
				}
				res.RigorErrorDetail = g.Description
				break
			}
		}

		// Fail closed if secret_scan or stub_detection did not run on a non-empty diff.
		if !in.State.Empty && !res.RigorError {
			if !hasGate(rigor.applied, dcverify.GateSecretScan) || !hasGate(rigor.applied, dcverify.GateStubDetection) {
				res.RigorError = true
				res.RigorErrorCode = RangeErrRequiredGatesAbsent
				res.RigorErrorDetail = fmt.Sprintf("required rigor gates missing from applied=%v", rigor.applied)
				gaps = append(gaps, Gap{
					ID:             StableGapID(taskID, "RIGOR-MISSING-GATES"),
					Severity:       "high",
					GapType:        "rigor_check_unavailable",
					TaskID:         taskID,
					Description:    "Required rigor gates (secret_scan, stub_detection) were not both reported as applied.",
					Evidence:       []string{res.RigorErrorDetail},
					RecommendedFix: "Upgrade or repair dcverify so both secret_scan and stub_detection run.",
					Blocking:       true,
				})
			} else if !hasGate(rigor.applied, dcverify.GateAssertFreeTest) {
				// Language with no AST stub checks (StubASTParsedFiles == 0):
				// listed as not applicable, NOT degraded.
				res.StubASTNotApplicable = true
			}
		}
	} else {
		// DiffTooLarge: coverage and rigor did not run (never a partial scan).
		res.CoverageSkippedReason = "diff exceeded size limit"
	}

	gaps = NormalizeGaps(gaps)
	status, passed, verdictDegraded := Verdict(gaps, meta)
	demoted := DemoteForMode(gaps, mode)

	for _, g := range demoted {
		if g.Blocking {
			res.BlockingGapCount++
			if gating.IsHardSafetyGap(g.GapType) {
				res.HardSafetyCount++
			}
		}
		if g.GapType == "stub_declared" {
			res.StubAllowedCount++
			res.StubAllowed = append(res.StubAllowed, g)
		}
	}

	res.Gaps = demoted
	res.Degraded = sortDegraded(res.Degraded)
	if len(verdictDegraded) > 0 {
		res.DegradedReasons = verdictDegraded
	} else if len(meta.DegradedReasons) > 0 {
		res.DegradedReasons = append([]string(nil), meta.DegradedReasons...)
	}

	switch {
	case res.RigorError:
		res.Status = "error"
		res.Passed = false
	case status == "blocked":
		res.Status = "blocked"
		res.Passed = false
	case len(res.Degraded) > 0 || status == "degraded":
		res.Status = "degraded"
		res.Passed = false
	default:
		res.Status = "verified"
		res.Passed = passed
	}
	return res
}

func hasGate(applied []string, want string) bool {
	for _, g := range applied {
		if g == want {
			return true
		}
	}
	return false
}

func copySorted(in []string) []string {
	if len(in) == 0 {
		return []string{}
	}
	out := append([]string(nil), in...)
	sort.Strings(out)
	return out
}

func sortDegraded(in []RangeDegraded) []RangeDegraded {
	if len(in) == 0 {
		return []RangeDegraded{}
	}
	out := append([]RangeDegraded(nil), in...)
	sort.Slice(out, func(i, j int) bool {
		if out[i].Code != out[j].Code {
			return out[i].Code < out[j].Code
		}
		if out[i].Path != out[j].Path {
			return out[i].Path < out[j].Path
		}
		return out[i].Detail < out[j].Detail
	})
	return out
}
