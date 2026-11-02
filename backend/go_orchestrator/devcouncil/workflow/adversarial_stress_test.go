package workflow

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// TestAdversarialTopoSortCycles checks multi-node cycles, self-loops, and disconnected cycles.
func TestAdversarialTopoSortCycles(t *testing.T) {
	// Case 1: Self-loop A -> A
	selfLoop := []StepDefinition{
		{ID: "step-a", DependsOn: []string{"step-a"}},
	}
	_, err := TopologicalSortSteps(selfLoop)
	if err == nil {
		t.Errorf("expected error for self-loop dependency, got nil")
	}

	// Case 2: 3-node cycle A -> B -> C -> A
	threeNodeCycle := []StepDefinition{
		{ID: "step-a", DependsOn: []string{"step-c"}},
		{ID: "step-b", DependsOn: []string{"step-a"}},
		{ID: "step-c", DependsOn: []string{"step-b"}},
	}
	_, err = TopologicalSortSteps(threeNodeCycle)
	if err == nil {
		t.Errorf("expected error for 3-node cycle, got nil")
	}

	// Case 3: Component cycle in larger DAG: X -> Y (ok), A -> B -> A (cycle)
	subgraphCycle := []StepDefinition{
		{ID: "step-x"},
		{ID: "step-y", DependsOn: []string{"step-x"}},
		{ID: "step-a", DependsOn: []string{"step-b"}},
		{ID: "step-b", DependsOn: []string{"step-a"}},
	}
	_, err = TopologicalSortSteps(subgraphCycle)
	if err == nil {
		t.Errorf("expected error for subgraph cycle, got nil")
	}

	// Case 4: Nonexistent dependency reference
	missingDep := []StepDefinition{
		{ID: "step-a", DependsOn: []string{"ghost-step"}},
	}
	_, err = TopologicalSortSteps(missingDep)
	if err == nil || !strings.Contains(err.Error(), "nonexistent dependency") {
		t.Errorf("expected nonexistent dependency error, got %v", err)
	}
}

// TestAdversarialMissingYAMLOnResume verifies error handling when definition file is absent.
func TestAdversarialMissingYAMLOnResume(t *testing.T) {
	tmpDir := t.TempDir()
	runID := "wf-missing-yaml-001"

	// Create paused state pointing to non-existent workflow def
	state := &RunState{
		RunID:        runID,
		WorkflowName: "nonexistent-def-file",
		Status:       "paused_at_gate",
		CurrentStep:  "step-gate",
		StartedAt:    nowISO(),
		UpdatedAt:    nowISO(),
		Variables:    make(map[string]string),
		StepResults:  make(map[string]StepResult),
	}
	if err := SaveState(tmpDir, state); err != nil {
		t.Fatal(err)
	}

	_, err := ResumeWorkflow(tmpDir, runID, "approve", nil)
	if err == nil {
		t.Fatalf("expected error when resuming with missing YAML definition, got nil")
	}
	if !strings.Contains(err.Error(), "failed to load definition") {
		t.Errorf("expected 'failed to load definition' error message, got: %v", err)
	}
}

// TestAdversarialInvalidStatusTransitions checks resumption from completed, failed, or unknown states.
func TestAdversarialInvalidStatusTransitions(t *testing.T) {
	tmpDir := t.TempDir()

	invalidStatuses := []string{"completed", "failed", "aborted", "unknown"}
	for _, status := range invalidStatuses {
		t.Run("status_"+status, func(t *testing.T) {
			runID := "wf-invalid-status-" + status
			state := &RunState{
				RunID:        runID,
				WorkflowName: "test-def",
				Status:       status,
				CurrentStep:  "step-1",
				StartedAt:    nowISO(),
				UpdatedAt:    nowISO(),
				Variables:    make(map[string]string),
				StepResults:  make(map[string]StepResult),
			}
			if err := SaveState(tmpDir, state); err != nil {
				t.Fatal(err)
			}

			_, err := ResumeWorkflow(tmpDir, runID, "continue", nil)
			if err == nil {
				t.Errorf("expected error resuming workflow in status '%s', got nil", status)
			}
			expectedSubstr := fmt.Sprintf("cannot resume workflow in status '%s'", status)
			if !strings.Contains(err.Error(), expectedSubstr) {
				t.Errorf("expected error containing '%s', got: %v", expectedSubstr, err)
			}
		})
	}
}

// TestAdversarialStepIterationBounds verifies that custom step MaxIterations and global limits hold.
func TestAdversarialStepIterationBounds(t *testing.T) {
	tmpDir := t.TempDir()

	// 1. Custom MaxIterations = 5
	customBoundDef := &WorkflowDefinition{
		Name: "custom-bound-flow",
		Steps: []StepDefinition{
			{
				ID:            "step-loop",
				Type:          StepTypeIf,
				Condition:     "true",
				ThenStep:      "step-loop",
				MaxIterations: 5,
			},
		},
	}

	state := &RunState{
		RunID:        NewRunID(),
		WorkflowName: customBoundDef.Name,
		Status:       "running",
		CurrentStep:  "step-loop",
		StartedAt:    nowISO(),
		UpdatedAt:    nowISO(),
		Variables:    make(map[string]string),
		StepResults:  make(map[string]StepResult),
	}

	finalState, err := ExecuteLoop(tmpDir, customBoundDef, state)
	if err == nil {
		t.Fatalf("expected error exceeding max_iterations (5), got nil")
	}
	if !strings.Contains(err.Error(), "exceeded max_iterations limit (5)") {
		t.Errorf("expected max_iterations limit (5) in error, got: %v", err)
	}
	if finalState.Status != "failed" {
		t.Errorf("expected status 'failed', got '%s'", finalState.Status)
	}

	// 2. Global iteration limit 1000 with 11 steps looping under their individual limits (e.g. 100 each)
	// 11 steps in loop, each with max_iterations 150, but total > 1000 should trigger global limit
	steps := make([]StepDefinition, 10)
	for i := 0; i < 10; i++ {
		nextID := fmt.Sprintf("step-%d", (i+1)%10)
		steps[i] = StepDefinition{
			ID:            fmt.Sprintf("step-%d", i),
			Type:          StepTypeIf,
			Condition:     "true",
			ThenStep:      nextID,
			MaxIterations: 200, // 10 * 200 = 2000 > 1000 global limit
		}
	}
	globalLimitDef := &WorkflowDefinition{
		Name:  "global-limit-flow",
		Steps: steps,
	}

	globalState := &RunState{
		RunID:        NewRunID(),
		WorkflowName: globalLimitDef.Name,
		Status:       "running",
		CurrentStep:  "step-0",
		StartedAt:    nowISO(),
		UpdatedAt:    nowISO(),
		Variables:    make(map[string]string),
		StepResults:  make(map[string]StepResult),
	}

	finalGlobalState, err := ExecuteLoop(tmpDir, globalLimitDef, globalState)
	if err == nil {
		t.Fatalf("expected global limit error, got nil")
	}
	if !strings.Contains(err.Error(), "global iteration limit (1000)") {
		t.Errorf("expected global iteration limit error, got: %v", err)
	}
	if finalGlobalState.Status != "failed" {
		t.Errorf("expected status 'failed', got '%s'", finalGlobalState.Status)
	}
}

// TestAdversarialGateRejectionDecision tests rejection case-insensitivity and whitespace handling.
func TestAdversarialGateRejectionDecision(t *testing.T) {
	tmpDir := t.TempDir()

	wfYAML := `
name: "gate-rejection-flow"
steps:
  - id: "step-gate"
    type: "gate"
    prompt: "Approve deployment?"
    allowed_decisions: ["approve", "reject"]
  - id: "step-deploy"
    type: "command"
    command: "echo Deployed"
`
	wfPath := filepath.Join(tmpDir, ".devcouncil", "workflows", "gate-rejection-flow.yaml")
	if err := os.MkdirAll(filepath.Dir(wfPath), 0755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(wfPath, []byte(wfYAML), 0644); err != nil {
		t.Fatal(err)
	}

	runID := "wf-gate-reject-001"
	pausedState := &RunState{
		RunID:        runID,
		WorkflowName: "gate-rejection-flow",
		Status:       "paused_at_gate",
		CurrentStep:  "step-gate",
		StartedAt:    nowISO(),
		UpdatedAt:    nowISO(),
		Variables:    make(map[string]string),
		StepResults: map[string]StepResult{
			"step-gate": {
				Status:           "waiting_decision",
				Prompt:           "Approve deployment?",
				AllowedDecisions: []string{"approve", "reject"},
				StartedAt:        nowISO(),
				PausedAt:         nowISO(),
			},
		},
	}
	if err := SaveState(tmpDir, pausedState); err != nil {
		t.Fatal(err)
	}

	// Test "  REJECT  " decision
	resState, err := ResumeWorkflow(tmpDir, runID, "  REJECT  ", nil)
	if err == nil {
		t.Fatalf("expected error on rejection decision, got nil")
	}
	if !strings.Contains(err.Error(), "rejected at gate step") {
		t.Errorf("expected rejection error, got: %v", err)
	}
	if resState.Status != "failed" {
		t.Errorf("expected status 'failed', got '%s'", resState.Status)
	}
	if res, ok := resState.StepResults["step-gate"]; !ok || res.Status != "rejected" {
		t.Errorf("expected step result status 'rejected', got: %+v", res)
	}
}
