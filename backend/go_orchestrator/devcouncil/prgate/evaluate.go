package prgate

import (
	"fmt"
	"sort"
	"strings"
	"unicode/utf8"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/devmap"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/gating"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/policy"
)

// FixedGapMessage returns the fixed, privacy-safe description for a gap_type.
// PR comments, SARIF message.text, and workflow annotations ONLY use these
// strings — never Gap.Description, Gap.Evidence, or Gap.RecommendedFix.
func FixedGapMessage(gapType string) string {
	switch gapType {
	case "security_risk":
		return "Potential secret or credential detected in added lines."
	case "stub_detected":
		return "Placeholder or stub implementation detected in added lines."
	case "stub_declared":
		return "Author-declared allow-stub marker present."
	case "assert_free_test":
		return "Added test contains no assertion."
	case "skipped_test":
		return "Added test is unconditionally skipped."
	case "runner_hijack":
		return "Potential test-runner bypass or hijack detected."
	case "lean_tcb":
		return "Lean 4 TCB soundness hole detected."
	case "low_substance":
		return "Diff has low substantive line ratio."
	case "diff_not_exercised":
		return "Added lines were not exercised by supplied coverage profile."
	case "rigor_check_unavailable":
		return "Required verification gate could not run."
	default:
		return "Verification finding detected."
	}
}

// FixedDegradedMessage returns the fixed, privacy-safe description for a
// verify.RangeDegraded code.
func FixedDegradedMessage(code string) string {
	switch code {
	case verify.DegradedDiffTooLarge:
		return "Diff exceeded verification size cap; partial scans are refused."
	case verify.DegradedGitlinkMoved:
		return "Submodule gitlink changed; submodule commit range was not scanned."
	case verify.DegradedBinaryDiffSuppressed:
		return "Diff was suppressed as binary on a text path (possible .gitattributes -diff)."
	case verify.DegradedSensitivePathChanged:
		return "Sensitive configuration or workflow file changed; manual review required."
	case verify.DegradedStubASTFallback:
		return "Stub AST parser degraded on one or more files; weaker line checks used."
	case verify.DegradedBinarySecretScanSkipped:
		return "Secret scan could not read one or more binary files."
	default:
		return "Verification check degraded and could not run completely."
	}
}

// Evaluate combines range verification and blast qualification into a single
// deterministic, fail-closed Report.
func Evaluate(in EvalInput) Report {
	mode, modeErr := verify.NormalizeRangeMode(in.Mode)
	if modeErr != nil {
		return errorReport(in, "invalid_mode", "devcouncil pr-gate: invalid gate mode")
	}

	if mode == "off" {
		return Report{
			Schema:          SchemaVersion,
			Outcome:         OutcomeSkipped,
			ExitCode:        ExitPass,
			ShouldPost:      false,
			StatusContext:   StatusContext,
			Mode:            "off",
			BaseSHA:         in.BaseSHA,
			HeadSHA:         in.HeadSHA,
			MergeBaseSHA:    in.MergeBaseSHA,
			GatesRun:        []string{},
			Coverage:        CoverageSummary{Measured: false, Status: "not measured"},
			Blast:           BlastQualification{AffectedTests: []string{}, Degraded: []DegradedItem{}, Notices: []NoticeItem{}},
			Findings:        []FindingItem{},
			StubAllowed:     []StubAllowedItem{},
			Degraded:        []DegradedItem{},
			Notices:         []NoticeItem{},
			SensitivePaths:  []string{},
			DependencyPaths: []string{},
		}
	}

	if in.CheckoutMismatch {
		return errorReport(in, "checkout_head_mismatch", "devcouncil pr-gate: checked-out HEAD does not match PR head SHA")
	}

	blastQ := QualifyBlast(in.Blast, in.BlastErr, in.HeadSHA)

	findings := make([]FindingItem, 0, len(in.Range.Gaps))
	stubAllowed := make([]StubAllowedItem, 0, in.Range.StubAllowedCount)
	blockingCount := 0
	advisoryCount := 0
	hardSafetyCount := 0

	for _, g := range in.Range.Gaps {
		path := ""
		if g.File != nil {
			path = *g.File
		}
		line := 0
		if g.Line != nil {
			line = *g.Line
		}
		if g.GapType == "stub_declared" {
			stubAllowed = append(stubAllowed, StubAllowedItem{
				Path: redactSecretPath(path),
				Line: line,
			})
		}
		item := FindingItem{
			ID:         g.ID,
			GapType:    g.GapType,
			Severity:   g.Severity,
			Blocking:   g.Blocking,
			HardSafety: gating.IsHardSafetyGap(g.GapType),
			Path:       redactSecretPath(path),
			Line:       line,
			Message:    FixedGapMessage(g.GapType),
		}
		// Only carry dcverify's already-redacted evidence on security_risk
		// findings for JSON consumers, bounded and withheld on secret paths.
		if g.GapType == "security_risk" && !policy.SecretPathSpelling(path) && len(g.Evidence) > 0 {
			item.EvidenceRedacted = boundUTF8(g.Evidence[0], MaxEvidenceBytes)
		}
		findings = append(findings, item)
		if g.Blocking {
			blockingCount++
			if item.HardSafety {
				hardSafetyCount++
			}
		} else {
			advisoryCount++
		}
	}

	sort.Slice(stubAllowed, func(i, j int) bool {
		if stubAllowed[i].Path != stubAllowed[j].Path {
			return stubAllowed[i].Path < stubAllowed[j].Path
		}
		return stubAllowed[i].Line < stubAllowed[j].Line
	})

	var degraded []DegradedItem
	for _, d := range in.Range.Degraded {
		degraded = append(degraded, DegradedItem{
			Code:    d.Code,
			Path:    redactSecretPath(d.Path),
			Message: FixedDegradedMessage(d.Code),
		})
	}
	for _, d := range blastQ.Degraded {
		degraded = append(degraded, DegradedItem{
			Code:    d.Code,
			Path:    redactSecretPath(d.Path),
			Message: d.Message,
		})
	}
	degraded = sortDegradedItems(degraded)

	var notices []NoticeItem
	for _, n := range blastQ.Notices {
		notices = append(notices, NoticeItem{
			Code:    n.Code,
			Path:    redactSecretPath(n.Path),
			Message: n.Message,
		})
	}
	for _, dep := range in.Range.DependencyFiles {
		notices = append(notices, NoticeItem{
			Code:    "dependency_manifest_changed",
			Path:    redactSecretPath(dep),
			Message: "Dependency manifest or lockfile changed.",
		})
	}
	if in.Range.StubASTNotApplicable {
		notices = append(notices, NoticeItem{
			Code:    "stub_ast_not_applicable",
			Message: "AST stub/test rigor checks are not applicable to the changed file types.",
		})
	}
	notices = sortNoticeItems(notices)

	gatesRun := append([]string(nil), in.Range.RigorApplied...)
	if gatesRun == nil {
		gatesRun = []string{}
	}
	sort.Strings(gatesRun)

	sensitive := redactPaths(in.Range.SensitiveFiles)
	deps := redactPaths(in.Range.DependencyFiles)

	cov := CoverageSummary{Measured: in.Range.CoverageMeasured, Status: "not measured"}
	if in.Range.CoverageMeasured {
		cov.Status = "measured"
	}

	rep := Report{
		Schema:          SchemaVersion,
		ShouldPost:      true,
		StatusContext:   StatusContext,
		Mode:            mode,
		BaseSHA:         in.BaseSHA,
		HeadSHA:         in.HeadSHA,
		MergeBaseSHA:    in.MergeBaseSHA,
		GatesRun:        gatesRun,
		Coverage:        cov,
		Blast:           blastQ,
		Findings:        findings,
		StubAllowed:     stubAllowed,
		Degraded:        degraded,
		Notices:         notices,
		SensitivePaths:  sensitive,
		DependencyPaths: deps,
		Summary: SummaryCounts{
			ChangedFiles:     len(in.Range.ChangedFiles),
			BlockingFindings: blockingCount,
			AdvisoryFindings: advisoryCount,
			HardSafety:       hardSafetyCount,
			StubAllowed:      len(stubAllowed),
			Degraded:         len(degraded),
			Notices:          len(notices),
			SensitivePaths:   len(sensitive),
			DependencyPaths:  len(deps),
			AffectedTests:    len(blastQ.AffectedTests),
		},
	}

	// Cross-check blast.ChangedFiles against range.ChangedFiles when blast
	// succeeded cleanly so a stale/partial blast cannot silently disagree with
	// git's commit range.
	blastChangedMismatch := blastQ.Available && !in.Range.RigorError && !blastQ.Error &&
		!changedFilesAgree(in.Range.ChangedFiles, in.Blast.ChangedFiles)

	// Strict fail-closed outcome & exit-code mapping.
	// Degraded NEVER maps to neutral or success.
	// Success also requires in.Range.Status == "verified" && in.Range.Passed
	// (defence in depth so the two verdicts can never disagree).
	switch {
	case in.Range.RigorError:
		rep.Outcome = OutcomeError
		rep.ExitCode = ExitError
		rep.StatusState = "error"
		rep.ErrorCode = in.Range.RigorErrorCode
		rep.StatusDescription = clampDescription(fmt.Sprintf("devcouncil pr-gate: verification error (%s)", safeWord(in.Range.RigorErrorCode)))
	case blastQ.Error:
		rep.Outcome = OutcomeError
		rep.ExitCode = ExitError
		rep.StatusState = "error"
		rep.ErrorCode = blastQ.ErrorCode
		rep.StatusDescription = clampDescription(fmt.Sprintf("devcouncil pr-gate: blast error (%s)", safeWord(blastQ.ErrorCode)))
	case blastChangedMismatch:
		rep.Outcome = OutcomeError
		rep.ExitCode = ExitError
		rep.StatusState = "error"
		rep.ErrorCode = "blast_changed_files_mismatch"
		rep.StatusDescription = clampDescription("devcouncil pr-gate: blast changed files disagree with git range")
	case blockingCount > 0:
		rep.Outcome = OutcomeFailure
		rep.ExitCode = ExitFail
		rep.StatusState = "failure"
		rep.StatusDescription = clampDescription(fmt.Sprintf("devcouncil pr-gate: %d blocking finding(s), %d degraded", blockingCount, len(degraded)))
	case len(degraded) > 0:
		rep.Outcome = OutcomeError
		rep.ExitCode = ExitError
		rep.StatusState = "error"
		rep.ErrorCode = "degraded"
		rep.StatusDescription = clampDescription(fmt.Sprintf("devcouncil pr-gate: degraded (%d incomplete check(s))", len(degraded)))
	case in.Range.Status != "verified" || !in.Range.Passed:
		rep.Outcome = OutcomeError
		rep.ExitCode = ExitError
		rep.StatusState = "error"
		rep.ErrorCode = "range_not_verified"
		rep.StatusDescription = clampDescription("devcouncil pr-gate: range verification did not reach verified state")
	default:
		rep.Outcome = OutcomeSuccess
		rep.ExitCode = ExitPass
		rep.StatusState = "success"
		rep.StatusDescription = clampDescription(fmt.Sprintf("devcouncil pr-gate: passed (%d file(s), %d advisory, %d allow-stub)", rep.Summary.ChangedFiles, advisoryCount, len(stubAllowed)))
	}

	return rep
}

func changedFilesAgree(rangeChanged []string, blastChanged []devmap.BlastChangedFile) bool {
	want := make(map[string]struct{}, len(rangeChanged))
	for _, p := range rangeChanged {
		if p != "" {
			want[p] = struct{}{}
		}
	}
	got := make(map[string]struct{}, len(blastChanged))
	for _, f := range blastChanged {
		if f.Path != "" {
			got[f.Path] = struct{}{}
		}
		if f.RenamedFrom != "" {
			got[f.RenamedFrom] = struct{}{}
		}
	}
	if len(want) != len(got) {
		return false
	}
	for p := range want {
		if _, ok := got[p]; !ok {
			return false
		}
	}
	return true
}

func errorReport(in EvalInput, code, desc string) Report {
	return Report{
		Schema:            SchemaVersion,
		Outcome:           OutcomeError,
		ExitCode:          ExitError,
		ShouldPost:        true,
		StatusContext:     StatusContext,
		StatusState:       "error",
		StatusDescription: clampDescription(desc),
		Mode:              in.Mode,
		BaseSHA:           in.BaseSHA,
		HeadSHA:           in.HeadSHA,
		MergeBaseSHA:      in.MergeBaseSHA,
		ErrorCode:         code,
		GatesRun:          []string{},
		Coverage:          CoverageSummary{Measured: false, Status: "not measured"},
		Blast:             BlastQualification{AffectedTests: []string{}, Degraded: []DegradedItem{}, Notices: []NoticeItem{}},
		Findings:          []FindingItem{},
		StubAllowed:       []StubAllowedItem{},
		Degraded:          []DegradedItem{},
		Notices:           []NoticeItem{},
		SensitivePaths:    []string{},
		DependencyPaths:   []string{},
	}
}

func redactSecretPath(p string) string {
	if p == "" {
		return ""
	}
	if policy.SecretPathSpelling(p) {
		return policy.WithheldSecretPath
	}
	return p
}

func redactPaths(paths []string) []string {
	if len(paths) == 0 {
		return []string{}
	}
	out := make([]string, 0, len(paths))
	for _, p := range paths {
		out = append(out, redactSecretPath(p))
	}
	sort.Strings(out)
	return out
}

func safeWord(s string) string {
	if s == "" || len(s) > 48 {
		return "unknown"
	}
	for i := 0; i < len(s); i++ {
		c := s[i]
		if !((c >= 'a' && c <= 'z') || (c >= '0' && c <= '9') || c == '_') {
			return "unknown"
		}
	}
	return s
}

func clampDescription(s string) string {
	if utf8.RuneCountInString(s) <= MaxStatusDescription {
		return s
	}
	runes := []rune(s)
	return string(runes[:MaxStatusDescription-1]) + "…"
}

func boundUTF8(s string, maxBytes int) string {
	s = strings.TrimSpace(s)
	if len(s) <= maxBytes {
		return s
	}
	cut := maxBytes
	for cut > 0 && !utf8.RuneStart(s[cut]) {
		cut--
	}
	return s[:cut] + "…"
}

func sortDegradedItems(in []DegradedItem) []DegradedItem {
	if len(in) == 0 {
		return []DegradedItem{}
	}
	out := append([]DegradedItem(nil), in...)
	sort.Slice(out, func(i, j int) bool {
		if out[i].Code != out[j].Code {
			return out[i].Code < out[j].Code
		}
		if out[i].Path != out[j].Path {
			return out[i].Path < out[j].Path
		}
		return out[i].Message < out[j].Message
	})
	return out
}

func sortNoticeItems(in []NoticeItem) []NoticeItem {
	if len(in) == 0 {
		return []NoticeItem{}
	}
	out := append([]NoticeItem(nil), in...)
	sort.Slice(out, func(i, j int) bool {
		if out[i].Code != out[j].Code {
			return out[i].Code < out[j].Code
		}
		if out[i].Path != out[j].Path {
			return out[i].Path < out[j].Path
		}
		return out[i].Message < out[j].Message
	})
	return out
}
