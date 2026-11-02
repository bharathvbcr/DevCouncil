package workflow

import "time"

// StepType enumerates supported workflow step kinds.
type StepType string

const (
	StepTypeCommand StepType = "command"
	StepTypePrompt  StepType = "prompt"
	StepTypeGate    StepType = "gate"
	StepTypeIf      StepType = "if"
	StepTypeSwitch  StepType = "switch"
	StepTypeWhile   StepType = "while"
	StepTypeFanOut  StepType = "fan-out"
	StepTypeSlot    StepType = "slot"
)

// WorkflowDefinition represents a parsed YAML workflow specification.
type WorkflowDefinition struct {
	Name        string            `json:"name" yaml:"name"`
	Description string            `json:"description,omitempty" yaml:"description,omitempty"`
	Variables   map[string]string `json:"variables,omitempty" yaml:"variables,omitempty"`
	Steps       []StepDefinition  `json:"steps" yaml:"steps"`
}

// StepDefinition represents an individual step in the DAG.
type StepDefinition struct {
	ID               string            `json:"id" yaml:"id"`
	Type             StepType          `json:"type" yaml:"type"`
	Description      string            `json:"description,omitempty" yaml:"description,omitempty"`
	Command          string            `json:"command,omitempty" yaml:"command,omitempty"`
	Prompt           string            `json:"prompt,omitempty" yaml:"prompt,omitempty"`
	AllowedDecisions []string          `json:"allowed_decisions,omitempty" yaml:"allowed_decisions,omitempty"`
	Condition        string            `json:"condition,omitempty" yaml:"condition,omitempty"`
	ThenStep         string            `json:"then_step,omitempty" yaml:"then_step,omitempty"`
	ElseStep         string            `json:"else_step,omitempty" yaml:"else_step,omitempty"`
	MaxIterations    int               `json:"max_iterations,omitempty" yaml:"max_iterations,omitempty"`
	Items            []string          `json:"items,omitempty" yaml:"items,omitempty"`
	Inputs           map[string]string `json:"inputs,omitempty" yaml:"inputs,omitempty"`
	DependsOn        []string          `json:"depends_on,omitempty" yaml:"depends_on,omitempty"`
}

// StepResult captures the execution output or decision for one step.
type StepResult struct {
	Status           string            `json:"status"` // completed, failed, skipped, waiting_decision
	ExitCode         int               `json:"exit_code,omitempty"`
	Output           string            `json:"output,omitempty"`
	Error            string            `json:"error,omitempty"`
	Prompt           string            `json:"prompt,omitempty"`
	AllowedDecisions []string          `json:"allowed_decisions,omitempty"`
	Decision         string            `json:"decision,omitempty"`
	DecisionInputs   map[string]string `json:"decision_inputs,omitempty"`
	StartedAt        string            `json:"started_at"`
	CompletedAt      string            `json:"completed_at,omitempty"`
	PausedAt         string            `json:"paused_at,omitempty"`
}

// RunState represents the complete persisted execution state of a workflow run.
type RunState struct {
	RunID        string                `json:"run_id"`
	WorkflowName string                `json:"workflow_name"`
	FeatureID    string                `json:"feature_id,omitempty"`
	Status       string                `json:"status"` // running, paused_at_gate, completed, failed
	CurrentStep  string                `json:"current_step"`
	StartedAt    string                `json:"started_at"`
	UpdatedAt    string                `json:"updated_at"`
	Variables    map[string]string     `json:"variables"`
	StepResults  map[string]StepResult `json:"step_results"`
}

func nowISO() string {
	return time.Now().UTC().Format(time.RFC3339)
}
