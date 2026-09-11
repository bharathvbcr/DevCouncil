// Package verify is the Phase-5 verifier: Report/Gap/NextAction shapes that
// match the Python MCP/CLI goldens, plus the gate orchestration that produces
// them. Pure diff∩scope / stub / secret / coverage work stays in dc-verify;
// this package owns task-shaped orchestration, command evidence, and
// persistence via dcstore.
package verify

import (
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
)

// Gap is one verification finding. Field names and nullability match the
// Python domain.gap.Gap / MCP payload.
type Gap struct {
	ID                         string   `json:"id"`
	Severity                   string   `json:"severity"`
	GapType                    string   `json:"gap_type"`
	RequirementID              *string  `json:"requirement_id"`
	TaskID                     string   `json:"task_id"`
	Description                string   `json:"description"`
	Evidence                   []string `json:"evidence"`
	RecommendedFix             string   `json:"recommended_fix"`
	Blocking                   bool     `json:"blocking"`
	File                       *string  `json:"file"`
	Line                       *int     `json:"line"`
	SuggestedCommand           *string  `json:"suggested_command"`
	AcceptanceCriterionID      *string  `json:"acceptance_criterion_id"`
	StdoutPath                 *string  `json:"stdout_path"`
	StderrPath                 *string  `json:"stderr_path"`
	ExpectedVerificationMethod *string  `json:"expected_verification_method"`
}

// NextAction is the machine-routable repair contract derived from a Gap.
type NextAction struct {
	GapID                      string   `json:"gap_id"`
	GapType                    string   `json:"gap_type"`
	Category                   string   `json:"category"`
	Severity                   string   `json:"severity"`
	Blocking                   bool     `json:"blocking"`
	Action                     string   `json:"action"`
	File                       *string  `json:"file"`
	Line                       *int     `json:"line"`
	AcceptanceCriterionID      *string  `json:"acceptance_criterion_id"`
	ExpectedVerificationMethod *string  `json:"expected_verification_method"`
	MissingEvidence            *string  `json:"missing_evidence"`
	SuggestedCommand           *string  `json:"suggested_command"`
	Evidence                   []string `json:"evidence"`
	StdoutPath                 *string  `json:"stdout_path"`
	StderrPath                 *string  `json:"stderr_path"`
}

// MCPResult is the leased verify_task success payload (golden mcp/verify).
type MCPResult struct {
	OK                    bool         `json:"ok"`
	TaskID                string       `json:"task_id"`
	Passed                bool         `json:"passed"`
	Status                string       `json:"status"`
	GateMode              string       `json:"gate_mode"`
	Difficulty            string       `json:"difficulty"`
	DiffEmpty             bool         `json:"diff_empty"`
	CoverageMeasured      bool         `json:"coverage_measured"`
	CoverageSkippedReason string       `json:"coverage_skipped_reason,omitempty"`
	CompilerActive        bool         `json:"compiler_active"`
	VerificationMode      string       `json:"verification_mode"`
	VerificationSkipped   bool         `json:"verification_skipped"`
	Sandbox               string       `json:"sandbox"`
	RigorApplied          []string     `json:"rigor_applied"`
	BlockingGaps          []Gap        `json:"blocking_gaps"`
	NextActions           []NextAction `json:"next_actions"`
	AdvisoryActions       []NextAction `json:"advisory_actions"`
	AllowedNextTools      []string     `json:"allowed_next_tools"`
}

// TaskCLIResult is one task entry inside the CLI verify --json envelope.
type TaskCLIResult struct {
	TaskID                string       `json:"task_id"`
	Status                string       `json:"status"`
	GateMode              string       `json:"gate_mode"`
	Difficulty            string       `json:"difficulty"`
	DiffEmpty             bool         `json:"diff_empty"`
	CoverageMeasured      bool         `json:"coverage_measured"`
	CoverageSkippedReason string       `json:"coverage_skipped_reason,omitempty"`
	CompilerActive        bool         `json:"compiler_active"`
	VerificationMode      string       `json:"verification_mode"`
	VerificationSkipped   bool         `json:"verification_skipped"`
	RigorApplied          []string     `json:"rigor_applied"`
	GapCount              int          `json:"gap_count"`
	BlockingGapCount      int          `json:"blocking_gap_count"`
	Gaps                  []Gap        `json:"gaps"`
	NextActions           []NextAction `json:"next_actions"`
	AdvisoryActions       []NextAction `json:"advisory_actions"`
}

// CLIResult is the `dev verify --json` / `devcouncil verify --json` envelope.
type CLIResult struct {
	OK                           bool            `json:"ok"`
	GateMode                     string          `json:"gate_mode"`
	ProcessedTasks               int             `json:"processed_tasks"`
	VerifiedTasks                int             `json:"verified_tasks"`
	BlockedTasks                 int             `json:"blocked_tasks"`
	CompletedWithoutVerification int             `json:"completed_without_verification"`
	TotalGaps                    int             `json:"total_gaps"`
	Tasks                        []TaskCLIResult `json:"tasks"`
	Error                        string          `json:"error,omitempty"`
}

// Input is everything orchestration needs from the outside world.
type Input struct {
	Root       string
	Task       *store.Task
	GateMode   string
	Sandbox    string
	Difficulty string

	// ChangedFiles and DiffContent come from git (or a test double).
	ChangedFiles []string
	DiffContent  string
	DiffEmpty    bool
	WorkPresent  bool

	// Commands are expected_tests, falling back to allowed_commands.
	Commands []string

	// RunCommand executes one verification command. Nil means commands are
	// skipped with reason "command runner unavailable".
	RunCommand func(command string) CommandOutcome
}

// CommandOutcome is the result of attempting to run one verification command.
type CommandOutcome struct {
	ExitCode int
	Summary  string
	Stdout   string
	Stderr   string
	TimedOut bool
	// Skipped is set when the runner refused to start the command (missing
	// tooling, not applicable). Never treat as pass.
	Skipped bool
	Reason  string
}

// strPtr helpers keep JSON nulls for unset optional fields.
func strPtr(s string) *string {
	if s == "" {
		return nil
	}
	return &s
}

func filePtr(s string) *string { return strPtr(s) }

func plannedExpectingChange(files []dc.PlannedFile) []string {
	out := make([]string, 0, len(files))
	for _, pf := range files {
		if pf.AllowedChange != dc.ChangeReadOnly {
			out = append(out, pf.Path)
		}
	}
	return out
}
