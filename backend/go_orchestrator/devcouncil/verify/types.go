// Package verify is the Phase-5 verifier: Report/Gap/NextAction shapes that
// match the Python MCP/CLI goldens, plus the gate orchestration that produces
// them. Pure diff∩scope / stub / secret / coverage work stays in dc-verify;
// this package owns task-shaped orchestration, command evidence, and
// persistence via dcstore.
package verify

import (
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/dcverify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/lappi"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
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
	OK                    bool     `json:"ok"`
	TaskID                string   `json:"task_id"`
	Passed                bool     `json:"passed"`
	Status                string   `json:"status"`
	GateMode              string   `json:"gate_mode"`
	GateModeOrigin        string   `json:"gate_mode_origin,omitempty"`
	GateModeError         string   `json:"gate_mode_error,omitempty"`
	Difficulty            string   `json:"difficulty"`
	DiffEmpty             bool     `json:"diff_empty"`
	CoverageMeasured      bool     `json:"coverage_measured"`
	CoverageSkippedReason string   `json:"coverage_skipped_reason,omitempty"`
	CompilerActive        bool     `json:"compiler_active"`
	VerificationMode      string   `json:"verification_mode"`
	VerificationSkipped   bool     `json:"verification_skipped"`
	Sandbox               string   `json:"sandbox"`
	RigorApplied          []string `json:"rigor_applied"`
	RigorSkippedReason    string   `json:"rigor_skipped_reason,omitempty"`
	// DegradedReasons names the checks that could not run or failed without
	// blocking when Status is "degraded". Degraded is never passed.
	DegradedReasons  []string     `json:"degraded_reasons,omitempty"`
	BlockingGaps     []Gap        `json:"blocking_gaps"`
	NextActions      []NextAction `json:"next_actions"`
	AdvisoryActions  []NextAction `json:"advisory_actions"`
	AllowedNextTools []string     `json:"allowed_next_tools"`
	CorrectionPath   string       `json:"correction_path,omitempty"`
}

// TaskCLIResult is one task entry inside the CLI verify --json envelope.
type TaskCLIResult struct {
	TaskID                string       `json:"task_id"`
	Status                string       `json:"status"`
	GateMode              string       `json:"gate_mode"`
	GateModeOrigin        string       `json:"gate_mode_origin,omitempty"`
	GateModeError         string       `json:"gate_mode_error,omitempty"`
	Difficulty            string       `json:"difficulty"`
	DiffEmpty             bool         `json:"diff_empty"`
	CoverageMeasured      bool         `json:"coverage_measured"`
	CoverageSkippedReason string       `json:"coverage_skipped_reason,omitempty"`
	CompilerActive        bool         `json:"compiler_active"`
	VerificationMode      string       `json:"verification_mode"`
	VerificationSkipped   bool         `json:"verification_skipped"`
	RigorApplied          []string     `json:"rigor_applied"`
	RigorSkippedReason    string       `json:"rigor_skipped_reason,omitempty"`
	DegradedReasons       []string     `json:"degraded_reasons,omitempty"`
	GapCount              int          `json:"gap_count"`
	BlockingGapCount      int          `json:"blocking_gap_count"`
	Gaps                  []Gap        `json:"gaps"`
	NextActions           []NextAction `json:"next_actions"`
	AdvisoryActions       []NextAction `json:"advisory_actions"`
}

// CLIResult is the `dev verify --json` / `devcouncil verify --json` envelope.
type CLIResult struct {
	OK             bool   `json:"ok"`
	GateMode       string `json:"gate_mode"`
	GateModeOrigin string `json:"gate_mode_origin,omitempty"`
	GateModeError  string `json:"gate_mode_error,omitempty"`
	ProcessedTasks int    `json:"processed_tasks"`
	VerifiedTasks  int    `json:"verified_tasks"`
	BlockedTasks   int    `json:"blocked_tasks"`
	// DegradedTasks counts tasks whose gates did not all run: never verified.
	DegradedTasks                int             `json:"degraded_tasks,omitempty"`
	CompletedWithoutVerification int             `json:"completed_without_verification"`
	TotalGaps                    int             `json:"total_gaps"`
	Tasks                        []TaskCLIResult `json:"tasks"`
	Error                        string          `json:"error,omitempty"`
}

// GateState is a resolved gate mode with its provenance. It is what the entry
// points (VerifyTask, RunCLI, the stop gate) take in place of a bare mode, so
// that an invalid config reaches the payload instead of being flattened to a
// mode string on the way.
type GateState struct {
	Mode       string
	Origin     string
	ConfigPath string
	Error      string
}

// GateConfigInvalid is the gap type for a gate config that could not be read.
const GateConfigInvalid = "gate_config_invalid"

// VerifyEnvConfigInvalid is the gap type for an unreadable verify.env_allow.
const VerifyEnvConfigInvalid = "verify_env_config_invalid"

// Input is everything orchestration needs from the outside world.
type Input struct {
	Root       string
	Task       *store.Task
	GateMode   string
	Difficulty string

	// GateOrigin, GateConfigPath and GateConfigError describe where GateMode
	// came from. An origin of "invalid" means the config could not be read as
	// a gate setting, GateMode is the enforce it fell back to, and Run adds a
	// blocking GateConfigInvalid gap naming the path and the error.
	GateOrigin      string
	GateConfigPath  string
	GateConfigError string

	// EnvPolicy is the environment the verification commands ran with; Run
	// records it in the evidence and reports an unreadable allowlist as a gap.
	EnvPolicy EnvPolicy

	// ChangedFiles and DiffContent come from git (or a test double).
	ChangedFiles []string
	DiffContent  string
	DiffEmpty    bool
	WorkPresent  bool
	// MovedGitlinks are the submodule gitlinks the change touches. The diff
	// carries no bytes for the commits a moved one moved across, so Run names
	// each as not scanned in the degraded reasons.
	MovedGitlinks []proc.GitlinkChange
	// ToolStateDiff is the tool-owned state the secret gate reads, apart from
	// DiffContent: the other gates never see it. ToolStateUnscanned is what it
	// could not carry, each entry a degraded reason. See toolstate.go.
	ToolStateDiff      string
	ToolStateUnscanned []string

	// Commands are expected_tests, falling back to allowed_commands.
	Commands []string

	// Requirements are the requirements the task links to, which carry the
	// verification method of each acceptance criterion it claims.
	// RequirementsErr is set when they could not be read, and then every
	// claimed criterion blocks (criteria.go).
	Requirements    store.LinkedRequirements
	RequirementsErr error
	// Reviews is what verify read to judge the task's llm_review criteria.
	// Left zero by a caller whose task claims none; a claimed llm_review
	// criterion with zero evidence blocks, it is never discharged.
	Reviews ReviewEvidence

	// Rigor runs the stub, secret and diff∩coverage gates by spawning
	// dcverify. Nil means they do not run, which is reported as
	// RigorSkippedReason and never as a clean rigor pass.
	Rigor *dcverify.Client
	// CoveragePath is a coverage profile (Go coverprofile or LCOV) for the same
	// change. Empty means the coverage gate does not run; the findings gates
	// still do.
	CoveragePath string

	// GitErr is set when the git reads that produce ChangedFiles and
	// DiffContent failed. Run reports it as a blocking rigor_check_unavailable
	// gap: the gates below would otherwise judge an empty diff, which reads as
	// a clean one.
	GitErr error
	// AllowNoRigor is the explicit opt-out for enforce mode without dcverify.
	// Without it, a run whose secret and stub scans never ran is degraded, not
	// verified.
	AllowNoRigor bool

	// Lappi asks the Lappi agent about each changed file and records the
	// exchange, as DEVCOUNCIL_LAPPI_ASK / DEVCOUNCIL_LAPPI_COLLECT say. Nil
	// means neither. It can only add advisory, non-blocking gaps.
	Lappi *lappi.Asker

	// RunCommand executes one verification command. Nil means commands are
	// skipped with reason "command runner unavailable".
	RunCommand func(command string) CommandOutcome
}

// WithGitState sets the inputs that come from one collection of the working
// tree: the change the gates judge, and whatever else the collection found.
// It is the one place the two are joined, so a caller that reads a GitState
// cannot forget part of it.
func (in *Input) WithGitState(state GitState) {
	in.ChangedFiles = state.Changed
	in.DiffContent = state.Diff
	in.DiffEmpty = state.Empty
	in.WorkPresent = !state.Empty
	in.MovedGitlinks = state.MovedGitlinks
	in.ToolStateDiff = state.ToolStateDiff
	in.ToolStateUnscanned = state.ToolStateUnscanned
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
