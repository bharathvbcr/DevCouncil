package workflow

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
)

// NewRunID creates a unique identifier for a workflow execution run.
func NewRunID() string {
	b := make([]byte, 4)
	_, _ = rand.Read(b)
	dateStr := time.Now().UTC().Format("20060102")
	return fmt.Sprintf("wf-run-%s-%s", dateStr, hex.EncodeToString(b))
}

// StateFilePath resolves the path where RunState is persisted.
func StateFilePath(projectRoot, runID string) string {
	return filepath.Join(projectRoot, ".devcouncil", "workflows", "runs", runID, "state.json")
}

// WorkflowDefPath resolves standard workflow YAML path in .devcouncil/workflows/<name>.yaml.
func WorkflowDefPath(projectRoot, nameOrPath string) string {
	if strings.HasSuffix(nameOrPath, ".yaml") || strings.HasSuffix(nameOrPath, ".json") {
		if filepath.IsAbs(nameOrPath) {
			return nameOrPath
		}
		if fi, err := os.Stat(filepath.Join(projectRoot, nameOrPath)); err == nil && !fi.IsDir() {
			return filepath.Join(projectRoot, nameOrPath)
		}
	}

	cand := filepath.Join(projectRoot, ".devcouncil", "workflows", nameOrPath+".yaml")
	if fi, err := os.Stat(cand); err == nil && !fi.IsDir() {
		return cand
	}

	candJSON := filepath.Join(projectRoot, ".devcouncil", "workflows", nameOrPath+".json")
	if fi, err := os.Stat(candJSON); err == nil && !fi.IsDir() {
		return candJSON
	}

	return filepath.Join(projectRoot, nameOrPath)
}

// SaveState atomically commits the RunState to disk.
func SaveState(projectRoot string, state *RunState) error {
	filePath := StateFilePath(projectRoot, state.RunID)
	dir := filepath.Dir(filePath)
	if err := os.MkdirAll(dir, 0755); err != nil {
		return fmt.Errorf("failed to create run state dir: %w", err)
	}

	state.UpdatedAt = nowISO()
	data, err := json.MarshalIndent(state, "", "  ")
	if err != nil {
		return fmt.Errorf("failed to serialize run state: %w", err)
	}

	tmpFile := filePath + ".tmp"
	if err := os.WriteFile(tmpFile, data, 0644); err != nil {
		return fmt.Errorf("failed to write tmp state: %w", err)
	}

	if err := os.Rename(tmpFile, filePath); err != nil {
		return fmt.Errorf("failed to atomically rename state file: %w", err)
	}

	return nil
}

// LoadState reads the RunState from disk.
func LoadState(projectRoot, runID string) (*RunState, error) {
	filePath := StateFilePath(projectRoot, runID)
	data, err := os.ReadFile(filePath)
	if err != nil {
		return nil, fmt.Errorf("run state not found for %s: %w", runID, err)
	}

	var state RunState
	if err := json.Unmarshal(data, &state); err != nil {
		return nil, fmt.Errorf("failed to parse run state: %w", err)
	}

	return &state, nil
}

// TopologicalSortSteps orders steps based on DependsOn dependencies using Kahn's algorithm.
// Rejects cyclic dependencies and dangling prerequisite references.
func TopologicalSortSteps(steps []StepDefinition) ([]StepDefinition, error) {
	stepMap := make(map[string]StepDefinition, len(steps))
	inDegree := make(map[string]int, len(steps))
	dependents := make(map[string][]string, len(steps))

	for _, s := range steps {
		stepMap[s.ID] = s
		inDegree[s.ID] = 0
	}

	for _, s := range steps {
		for _, dep := range s.DependsOn {
			if _, exists := stepMap[dep]; !exists {
				return nil, fmt.Errorf("step '%s' references nonexistent dependency '%s'", s.ID, dep)
			}
			dependents[dep] = append(dependents[dep], s.ID)
			inDegree[s.ID]++
		}
	}

	var queue []string
	for _, s := range steps {
		if inDegree[s.ID] == 0 {
			queue = append(queue, s.ID)
		}
	}

	var result []StepDefinition
	for len(queue) > 0 {
		currID := queue[0]
		queue = queue[1:]
		result = append(result, stepMap[currID])

		for _, depID := range dependents[currID] {
			inDegree[depID]--
			if inDegree[depID] == 0 {
				queue = append(queue, depID)
			}
		}
	}

	if len(result) != len(steps) {
		return nil, fmt.Errorf("workflow cycle detected: cyclic dependency among steps")
	}

	return result, nil
}

// StartWorkflow initializes and runs a workflow until pause, completion, or failure.
func StartWorkflow(projectRoot string, def *WorkflowDefinition, initialVars map[string]string, featureID string) (*RunState, error) {
	if def == nil || len(def.Steps) == 0 {
		return nil, fmt.Errorf("workflow has no steps")
	}

	hasDeps := false
	for _, s := range def.Steps {
		if len(s.DependsOn) > 0 {
			hasDeps = true
			break
		}
	}
	if hasDeps {
		sorted, err := TopologicalSortSteps(def.Steps)
		if err != nil {
			return nil, err
		}
		def.Steps = sorted
	}

	runID := NewRunID()
	vars := make(map[string]string)
	for k, v := range def.Variables {
		vars[k] = v
	}
	for k, v := range initialVars {
		vars[k] = v
	}
	if featureID != "" {
		vars["feature_id"] = featureID
	}

	state := &RunState{
		RunID:        runID,
		WorkflowName: def.Name,
		FeatureID:    featureID,
		Status:       "running",
		CurrentStep:  def.Steps[0].ID,
		StartedAt:    nowISO(),
		UpdatedAt:    nowISO(),
		Variables:    vars,
		StepResults:  make(map[string]StepResult),
	}

	if err := SaveState(projectRoot, state); err != nil {
		return nil, err
	}

	return ExecuteLoop(projectRoot, def, state)
}

// ResumeWorkflow resumes a workflow paused at a gate or interrupted in running status.
func ResumeWorkflow(projectRoot, runID, decision string, decisionInputs map[string]string) (*RunState, error) {
	state, err := LoadState(projectRoot, runID)
	if err != nil {
		return nil, err
	}

	if state.Status != "paused_at_gate" && state.Status != "running" {
		return nil, fmt.Errorf("cannot resume workflow in status '%s' (expected 'paused_at_gate' or 'running')", state.Status)
	}

	defPath := WorkflowDefPath(projectRoot, state.WorkflowName)
	def, err := ParseWorkflow(defPath)
	if err != nil {
		return nil, fmt.Errorf("failed to load definition for resume: %w", err)
	}

	// If resuming an interrupted 'running' workflow, continue loop directly
	if state.Status == "running" {
		return ExecuteLoop(projectRoot, def, state)
	}

	// Update the gate step with the decision
	currID := state.CurrentStep
	res, ok := state.StepResults[currID]
	if !ok {
		res = StepResult{StartedAt: nowISO()}
	}

	res.Decision = decision
	res.DecisionInputs = decisionInputs
	res.CompletedAt = nowISO()

	// Check if rejected
	decClean := strings.ToLower(strings.TrimSpace(decision))
	if decClean == "reject" {
		res.Status = "rejected"
		state.StepResults[currID] = res
		state.Status = "failed"
		state.UpdatedAt = nowISO()
		_ = SaveState(projectRoot, state)
		return state, fmt.Errorf("workflow rejected at gate step %s", currID)
	}

	res.Status = "completed"
	state.StepResults[currID] = res

	// Advance to next step
	nextStepID := findNextStep(def, currID)
	if nextStepID == "" {
		state.Status = "completed"
		state.UpdatedAt = nowISO()
		_ = SaveState(projectRoot, state)
		return state, nil
	}

	state.CurrentStep = nextStepID
	state.Status = "running"
	if err := SaveState(projectRoot, state); err != nil {
		return nil, err
	}

	return ExecuteLoop(projectRoot, def, state)
}

// ExecuteLoop iterates through steps until a gate pause, completion, or error.
func ExecuteLoop(projectRoot string, def *WorkflowDefinition, state *RunState) (*RunState, error) {
	ctx := context.Background()
	stepIterations := make(map[string]int)
	totalIterations := 0

	for {
		if state.CurrentStep == "" {
			state.Status = "completed"
			break
		}

		step := findStep(def, state.CurrentStep)
		if step == nil {
			state.Status = "failed"
			_ = SaveState(projectRoot, state)
			return state, fmt.Errorf("step %s not found in workflow definition", state.CurrentStep)
		}

		// Enforce MaxIterations bound on cyclic/looping steps
		stepIterations[step.ID]++
		maxIter := step.MaxIterations
		if maxIter <= 0 {
			maxIter = 100 // Safe default bounding loop jumps
		}
		if stepIterations[step.ID] > maxIter {
			state.Status = "failed"
			_ = SaveState(projectRoot, state)
			return state, fmt.Errorf("step %s exceeded max_iterations limit (%d)", step.ID, maxIter)
		}

		totalIterations++
		if totalIterations > 1000 {
			state.Status = "failed"
			_ = SaveState(projectRoot, state)
			return state, fmt.Errorf("workflow execution exceeded global iteration limit (1000)")
		}

		switch step.Type {
		case StepTypeGate:
			state.Status = "paused_at_gate"
			state.StepResults[step.ID] = StepResult{
				Status:           "waiting_decision",
				Prompt:           step.Prompt,
				AllowedDecisions: step.AllowedDecisions,
				StartedAt:        nowISO(),
				PausedAt:         nowISO(),
			}
			_ = SaveState(projectRoot, state)
			return state, nil

		case StepTypeCommand:
			cmdStr := interpolate(step.Command, state.Variables)
			res := StepResult{
				StartedAt: nowISO(),
			}

			cmd := exec.CommandContext(ctx, "sh", "-c", cmdStr)
			cmd.Dir = projectRoot
			proc.EnforceSubprocessInvariants(cmd)
			outBuf := &proc.CappedBuffer{Limit: 2 << 20}
			cmd.Stdout = outBuf
			cmd.Stderr = outBuf
			runErr, timedOut := proc.RunBounded(ctx, cmd.Run)
			err := runErr
			if timedOut && err == nil {
				err = ctx.Err()
			}
			res.CompletedAt = nowISO()
			res.Output = strings.TrimSpace(outBuf.String())

			if err != nil {
				res.Status = "failed"
				res.Error = err.Error()
				if exitErr, ok := err.(*exec.ExitError); ok {
					res.ExitCode = exitErr.ExitCode()
				} else {
					res.ExitCode = 1
				}
				state.StepResults[step.ID] = res
				state.Status = "failed"
				_ = SaveState(projectRoot, state)
				return state, fmt.Errorf("command in step %s failed: %w (output: %s)", step.ID, err, res.Output)
			}

			res.Status = "completed"
			res.ExitCode = 0
			state.StepResults[step.ID] = res

			state.CurrentStep = findNextStep(def, step.ID)

		case StepTypePrompt:
			state.StepResults[step.ID] = StepResult{
				Status:      "completed",
				Prompt:      step.Prompt,
				StartedAt:   nowISO(),
				CompletedAt: nowISO(),
			}
			state.CurrentStep = findNextStep(def, step.ID)

		case StepTypeIf, StepTypeSwitch:
			// Evaluate condition
			cond := interpolate(step.Condition, state.Variables)
			matched := evalCondition(cond, state)
			if matched && step.ThenStep != "" {
				state.CurrentStep = step.ThenStep
			} else if !matched && step.ElseStep != "" {
				state.CurrentStep = step.ElseStep
			} else {
				state.CurrentStep = findNextStep(def, step.ID)
			}
			state.StepResults[step.ID] = StepResult{
				Status:      "completed",
				Output:      fmt.Sprintf("condition matched: %v", matched),
				StartedAt:   nowISO(),
				CompletedAt: nowISO(),
			}

		case StepTypeSlot:
			state.StepResults[step.ID] = StepResult{
				Status:      "completed",
				Output:      "slot pass-through",
				StartedAt:   nowISO(),
				CompletedAt: nowISO(),
			}
			state.CurrentStep = findNextStep(def, step.ID)

		default:
			// Default pass-through
			state.StepResults[step.ID] = StepResult{
				Status:      "completed",
				StartedAt:   nowISO(),
				CompletedAt: nowISO(),
			}
			state.CurrentStep = findNextStep(def, step.ID)
		}

		if err := SaveState(projectRoot, state); err != nil {
			return nil, err
		}
	}

	state.Status = "completed"
	_ = SaveState(projectRoot, state)
	return state, nil
}

func findStep(def *WorkflowDefinition, id string) *StepDefinition {
	for i := range def.Steps {
		if def.Steps[i].ID == id {
			return &def.Steps[i]
		}
	}
	return nil
}

func findNextStep(def *WorkflowDefinition, currentID string) string {
	for i := range def.Steps {
		if def.Steps[i].ID == currentID {
			if i+1 < len(def.Steps) {
				return def.Steps[i+1].ID
			}
			return ""
		}
	}
	return ""
}

func interpolate(tmpl string, vars map[string]string) string {
	out := tmpl
	for k, v := range vars {
		out = strings.ReplaceAll(out, "{{ "+k+" }}", v)
		out = strings.ReplaceAll(out, "{{"+k+"}}", v)
		out = strings.ReplaceAll(out, "$"+k, v)
	}
	return out
}

func evalCondition(cond string, state *RunState) bool {
	cond = strings.TrimSpace(cond)
	if cond == "" || cond == "true" {
		return true
	}
	if cond == "false" {
		return false
	}
	if strings.Contains(cond, "==") {
		parts := strings.SplitN(cond, "==", 2)
		left := strings.Trim(strings.TrimSpace(parts[0]), `"'`)
		right := strings.Trim(strings.TrimSpace(parts[1]), `"'`)
		return left == right
	}
	return true
}
