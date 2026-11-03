package workflow

import (
	"os"
	"path/filepath"
	"testing"
	"time"
)

// TestAdversarialCycleDetection verifies behavior when a workflow definition
// contains a cycle between steps (e.g., via ThenStep / condition loops).
// An empirical challenger checks whether ExecuteLoop detects cycles / bounds iterations
// or hangs in an infinite loop.
func TestAdversarialCycleDetection(t *testing.T) {
	tmpDir := t.TempDir()

	// Workflow with mutual loop: step-a -> step-b -> step-a
	cyclicDef := &WorkflowDefinition{
		Name: "cyclic-workflow",
		Steps: []StepDefinition{
			{
				ID:        "step-a",
				Type:      StepTypeIf,
				Condition: "true",
				ThenStep:  "step-b",
			},
			{
				ID:        "step-b",
				Type:      StepTypeIf,
				Condition: "true",
				ThenStep:  "step-a",
			},
		},
	}

	state := &RunState{
		RunID:        NewRunID(),
		WorkflowName: cyclicDef.Name,
		Status:       "running",
		CurrentStep:  "step-a",
		StartedAt:    nowISO(),
		UpdatedAt:    nowISO(),
		Variables:    make(map[string]string),
		StepResults:  make(map[string]StepResult),
	}

	done := make(chan error, 1)
	go func() {
		// Run ExecuteLoop in goroutine
		_, err := ExecuteLoop(tmpDir, cyclicDef, state)
		done <- err
	}()

	select {
	case err := <-done:
		// If it terminates, did it detect cycle or max iterations?
		t.Logf("ExecuteLoop terminated with: %v", err)
	// The bounded loop takes ~100 ms idle (100 iterations, each saving state),
	// but went past 500 ms inside a loaded `go test ./...`. A real infinite
	// loop never returns, so a wide window still catches it.
	case <-time.After(10 * time.Second):
		t.Errorf("FAIL / BUG: ExecuteLoop hung in an infinite loop on cyclic workflow definition (no cycle detection or max iteration bound)")
	}
}

// TestAdversarialResumeInterruptedRunningWorkflow verifies whether a workflow
// whose process was terminated (SIGKILL/power interruption) during status 'running'
// can be resumed.
func TestAdversarialResumeInterruptedRunningWorkflow(t *testing.T) {
	tmpDir := t.TempDir()

	// Write definition file to .devcouncil/workflows/interrupted-flow.yaml
	wfYAML := `
name: "interrupted-flow"
steps:
  - id: "step-init"
    type: "command"
    command: "echo Initializing"
  - id: "step-heavy-build"
    type: "command"
    command: "echo Building heavy target"
  - id: "step-finalize"
    type: "command"
    command: "echo Finalizing"
`
	wfPath := filepath.Join(tmpDir, ".devcouncil", "workflows", "interrupted-flow.yaml")
	if err := os.MkdirAll(filepath.Dir(wfPath), 0755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(wfPath, []byte(wfYAML), 0644); err != nil {
		t.Fatal(err)
	}

	runID := "wf-crashed-001"
	crashedState := &RunState{
		RunID:        runID,
		WorkflowName: "interrupted-flow",
		Status:       "running", // Left in 'running' when process died
		CurrentStep:  "step-heavy-build",
		StartedAt:    nowISO(),
		UpdatedAt:    nowISO(),
		Variables:    map[string]string{"env": "staging"},
		StepResults: map[string]StepResult{
			"step-init": {
				Status:    "completed",
				StartedAt: nowISO(),
			},
		},
	}

	if err := SaveState(tmpDir, crashedState); err != nil {
		t.Fatalf("failed to save initial state: %v", err)
	}

	// Resuming the crashed workflow should pick up from 'step-heavy-build' and run to completion
	resumedState, err := ResumeWorkflow(tmpDir, runID, "continue", nil)
	if err != nil {
		t.Fatalf("expected successful resumption of running workflow, got error: %v", err)
	}

	if resumedState.Status != "completed" {
		t.Fatalf("expected resumed workflow status 'completed', got '%s'", resumedState.Status)
	}

	if res, ok := resumedState.StepResults["step-heavy-build"]; !ok || res.Status != "completed" {
		t.Fatalf("expected step-heavy-build to be completed upon resumption, got: %+v", res)
	}
	if res, ok := resumedState.StepResults["step-finalize"]; !ok || res.Status != "completed" {
		t.Fatalf("expected step-finalize to be completed upon resumption, got: %+v", res)
	}
}

// TestAdversarialDanglingStepJump verifies behavior when ThenStep points to a non-existent step ID.
func TestAdversarialDanglingStepJump(t *testing.T) {
	tmpDir := t.TempDir()

	danglingDef := &WorkflowDefinition{
		Name: "dangling-step-workflow",
		Steps: []StepDefinition{
			{
				ID:        "step-start",
				Type:      StepTypeIf,
				Condition: "true",
				ThenStep:  "ghost-step-does-not-exist",
			},
		},
	}

	state := &RunState{
		RunID:        NewRunID(),
		WorkflowName: danglingDef.Name,
		Status:       "running",
		CurrentStep:  "step-start",
		StartedAt:    nowISO(),
		UpdatedAt:    nowISO(),
		Variables:    make(map[string]string),
		StepResults:  make(map[string]StepResult),
	}

	finalState, err := ExecuteLoop(tmpDir, danglingDef, state)
	if err == nil {
		t.Fatalf("expected error on jump to non-existent step, got nil")
	}
	if finalState.Status != "failed" {
		t.Errorf("expected final status 'failed', got '%s'", finalState.Status)
	}
	t.Logf("Handled correctly: %v", err)
}

// TestAdversarialStateAtomicityUnderRapidUpdates verifies atomic state persistence.
func TestAdversarialStateAtomicityUnderRapidUpdates(t *testing.T) {
	tmpDir := t.TempDir()
	runID := "wf-atomic-001"
	state := &RunState{
		RunID:        runID,
		WorkflowName: "atomic-flow",
		Status:       "running",
		StartedAt:    nowISO(),
		Variables:    make(map[string]string),
		StepResults:  make(map[string]StepResult),
	}

	for i := 0; i < 50; i++ {
		state.CurrentStep = filepath.Join("step", string(rune('0'+(i%10))))
		state.StepResults[state.CurrentStep] = StepResult{
			Status:    "completed",
			Output:    "data",
			StartedAt: nowISO(),
		}
		if err := SaveState(tmpDir, state); err != nil {
			t.Fatalf("iteration %d: SaveState failed: %v", i, err)
		}
		loaded, err := LoadState(tmpDir, runID)
		if err != nil {
			t.Fatalf("iteration %d: LoadState failed: %v", i, err)
		}
		if loaded.CurrentStep != state.CurrentStep {
			t.Fatalf("iteration %d: corrupted state read", i)
		}
	}
}
