// Package prgate implements the pure, deterministic evaluation and rendering
// layer for `devcouncil pr-gate`.
//
// Security and privacy invariants:
//   - Outcomes are strictly `success`, `failure`, `error` (including degraded),
//     or `skipped` (mode=off, posts nothing). Degraded NEVER maps to `neutral`.
//   - Exit codes: 0 (pass/skipped), 1 (failure), 2 (error/degraded), 64 (usage).
//   - Markdown PR comments, SARIF message.text, and workflow annotations emit
//     ONLY fixed per-gate vocabulary, counts, and escaped repository paths —
//     never symbol names, owner emails, Gap.Description, Gap.Evidence,
//     Gap.RecommendedFix, or subprocess stderr.
//   - Output is deterministic: no timestamps, sorted lists, hard caps on
//     annotations (<= 50) and comment length (< 65,536 bytes).
package prgate

import (
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/devmap"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
)

const (
	// SchemaVersion is the JSON schema identifier for `devcouncil pr-gate`.
	SchemaVersion = "devcouncil.pr_gate/v1"

	// StatusContext is the dedicated GitHub commit status context for PR Gate.
	// Distinct from GitPulse's `devcouncil/verify` so they never overwrite each other.
	StatusContext = "devcouncil/pr-gate"

	// StickyMarker is the hidden HTML marker identifying the PR Gate sticky comment.
	// Callers must match BOTH this marker AND the bot author when updating comments.
	StickyMarker = "<!-- devcouncil:pr-gate -->"

	OutcomeSuccess = "success"
	OutcomeFailure = "failure"
	OutcomeError   = "error"
	OutcomeSkipped = "skipped"

	ExitPass  = 0
	ExitFail  = 1
	ExitError = 2
	ExitUsage = 64

	MaxAnnotations       = 50
	MaxCommentBytes      = 65000 // strictly below GitHub's 65,536-char comment limit
	MaxStatusDescription = 140   // GitHub commit status description limit
	MaxEvidenceBytes     = 256
	MaxMarkdownRows      = 100
)

// EvalInput holds the pre-collected verification and blast inputs for Evaluate.
type EvalInput struct {
	Mode         string
	BaseSHA      string
	HeadSHA      string
	MergeBaseSHA string
	Range        verify.RangeResult
	Blast        *devmap.Blast
	BlastErr     error
	// CheckoutMismatch is true when the checked-out HEAD does not equal HeadSHA.
	CheckoutMismatch bool
}

// BlastQualification is the qualified blast summary and its completeness signals.
type BlastQualification struct {
	Available         bool           `json:"available"`
	Complete          bool           `json:"complete"`
	HeadSHA           string         `json:"head_sha,omitempty"`
	Error             bool           `json:"error,omitempty"`
	ErrorCode         string         `json:"error_code,omitempty"`
	ChangedFilesCount int            `json:"changed_files_count"`
	SeedsCount        int            `json:"seeds_count"`
	ImpactedCount     int            `json:"impacted_count"`
	AffectedTests     []string       `json:"affected_tests"`
	UnattributedCount int            `json:"unattributed_count"`
	Degraded          []DegradedItem `json:"degraded"`
	Notices           []NoticeItem   `json:"notices"`
}

// FindingItem is one privacy-safe finding in the PR Gate report.
type FindingItem struct {
	ID               string `json:"id"`
	GapType          string `json:"gap_type"`
	Severity         string `json:"severity"`
	Blocking         bool   `json:"blocking"`
	HardSafety       bool   `json:"hard_safety"`
	Path             string `json:"path,omitempty"`
	Line             int    `json:"line,omitempty"`
	Message          string `json:"message"`
	EvidenceRedacted string `json:"evidence_redacted,omitempty"`
}

// StubAllowedItem records an author-controlled `allow-stub:` marker.
type StubAllowedItem struct {
	Path string `json:"path,omitempty"`
	Line int    `json:"line,omitempty"`
}

// DegradedItem is one structured reason the PR Gate could not verify completely.
type DegradedItem struct {
	Code    string `json:"code"`
	Path    string `json:"path,omitempty"`
	Message string `json:"message"`
}

// NoticeItem is one non-degrading informational notice (e.g. blast depth cap,
// outside_every_symbol ranges, dependency manifests, or language N/A).
type NoticeItem struct {
	Code    string `json:"code"`
	Path    string `json:"path,omitempty"`
	Message string `json:"message"`
}

// CoverageSummary summarizes diff coverage status (never measured as pass when absent).
type CoverageSummary struct {
	Measured bool   `json:"measured"`
	Status   string `json:"status"`
}

// SummaryCounts holds aggregate counts rendered in JSON, Markdown, and status descriptions.
type SummaryCounts struct {
	ChangedFiles     int `json:"changed_files"`
	BlockingFindings int `json:"blocking_findings"`
	AdvisoryFindings int `json:"advisory_findings"`
	HardSafety       int `json:"hard_safety"`
	StubAllowed      int `json:"stub_allowed"`
	Degraded         int `json:"degraded"`
	Notices          int `json:"notices"`
	SensitivePaths   int `json:"sensitive_paths"`
	DependencyPaths  int `json:"dependency_paths"`
	AffectedTests    int `json:"affected_tests"`
}

// Report is the canonical `devcouncil.pr_gate/v1` deterministic JSON payload.
type Report struct {
	Schema            string             `json:"schema"`
	Outcome           string             `json:"outcome"`
	ExitCode          int                `json:"exit_code"`
	ShouldPost        bool               `json:"should_post"`
	StatusContext     string             `json:"status_context"`
	StatusState       string             `json:"status_state,omitempty"`
	StatusDescription string             `json:"status_description,omitempty"`
	Mode              string             `json:"mode"`
	BaseSHA           string             `json:"base_sha,omitempty"`
	HeadSHA           string             `json:"head_sha,omitempty"`
	MergeBaseSHA      string             `json:"merge_base_sha,omitempty"`
	ErrorCode         string             `json:"error_code,omitempty"`
	Summary           SummaryCounts      `json:"summary"`
	GatesRun          []string           `json:"gates_run"`
	Coverage          CoverageSummary    `json:"coverage"`
	Blast             BlastQualification `json:"blast"`
	Findings          []FindingItem      `json:"findings"`
	StubAllowed       []StubAllowedItem  `json:"stub_allowed"`
	Degraded          []DegradedItem     `json:"degraded"`
	Notices           []NoticeItem       `json:"notices"`
	SensitivePaths    []string           `json:"sensitive_paths"`
	DependencyPaths   []string           `json:"dependency_paths"`
}
