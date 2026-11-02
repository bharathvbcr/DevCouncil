package workflow

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestWorkflowLifecyclePauseAndResume(t *testing.T) {
	tmpDir := t.TempDir()

	workflowYAML := `
name: test-feature-pipeline
description: Pipeline for testing pause-at-gate and resumption
variables:
  msg: hello-world
steps:
  - id: step-one
    type: command
    command: "echo {{ msg }}"
  - id: gate-approval
    type: gate
    prompt: "Approve deployment to staging"
    allowed_decisions:
      - approve
      - reject
  - id: step-two
    type: command
    command: "echo Deployment completed"
`

	wfPath := filepath.Join(tmpDir, ".devcouncil", "workflows", "test-feature-pipeline.yaml")
	if err := os.MkdirAll(filepath.Dir(wfPath), 0755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(wfPath, []byte(workflowYAML), 0644); err != nil {
		t.Fatal(err)
	}

	def, err := ParseWorkflow(wfPath)
	if err != nil {
		t.Fatalf("failed to parse workflow: %v", err)
	}
	if len(def.Steps) != 3 {
		t.Fatalf("expected 3 steps, got %d", len(def.Steps))
	}

	// 1. Start Workflow -> should run step-one and pause at gate-approval
	state, err := StartWorkflow(tmpDir, def, map[string]string{"msg": "resumable-engine"}, "001-test")
	if err != nil {
		t.Fatalf("start workflow failed: %v", err)
	}

	if state.Status != "paused_at_gate" {
		t.Fatalf("expected status 'paused_at_gate', got '%s'", state.Status)
	}
	if state.CurrentStep != "gate-approval" {
		t.Fatalf("expected current step 'gate-approval', got '%s'", state.CurrentStep)
	}

	// Check that state was persisted to disk
	loaded, err := LoadState(tmpDir, state.RunID)
	if err != nil {
		t.Fatalf("failed to load state from disk: %v", err)
	}
	if loaded.Status != "paused_at_gate" {
		t.Errorf("expected loaded state to be paused_at_gate, got %s", loaded.Status)
	}
	step1Res := loaded.StepResults["step-one"]
	if step1Res.Status != "completed" || step1Res.Output != "resumable-engine" {
		t.Errorf("step-one unexpected result: %+v", step1Res)
	}

	// 2. Resume Workflow with 'approve'
	resumed, err := ResumeWorkflow(tmpDir, state.RunID, "approve", map[string]string{"auditor": "worker_impl_m1"})
	if err != nil {
		t.Fatalf("resume workflow failed: %v", err)
	}

	if resumed.Status != "completed" {
		t.Fatalf("expected resumed status 'completed', got '%s'", resumed.Status)
	}

	// Verify gate step recorded decision
	gateRes := resumed.StepResults["gate-approval"]
	if gateRes.Status != "completed" || gateRes.Decision != "approve" {
		t.Errorf("unexpected gate result: %+v", gateRes)
	}

	// Verify step-two completed
	step2Res := resumed.StepResults["step-two"]
	if step2Res.Status != "completed" || step2Res.Output != "Deployment completed" {
		t.Errorf("unexpected step2 result: %+v", step2Res)
	}
}

func TestWorkflowRejectionAtGate(t *testing.T) {
	tmpDir := t.TempDir()

	workflowYAML := `
name: test-reject-pipeline
steps:
  - id: gate-check
    type: gate
    prompt: "Approve or reject"
    allowed_decisions:
      - approve
      - reject
  - id: never-reached
    type: command
    command: "exit 1"
`

	wfPath := filepath.Join(tmpDir, ".devcouncil", "workflows", "test-reject-pipeline.yaml")
	if err := os.MkdirAll(filepath.Dir(wfPath), 0755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(wfPath, []byte(workflowYAML), 0644); err != nil {
		t.Fatal(err)
	}

	def, err := ParseWorkflow(wfPath)
	if err != nil {
		t.Fatal(err)
	}

	state, err := StartWorkflow(tmpDir, def, nil, "")
	if err != nil {
		t.Fatal(err)
	}

	_, resumeErr := ResumeWorkflow(tmpDir, state.RunID, "reject", nil)
	if resumeErr == nil {
		t.Fatalf("expected error on rejection, got nil")
	}

	finalState, err := LoadState(tmpDir, state.RunID)
	if err != nil {
		t.Fatal(err)
	}
	if finalState.Status != "failed" {
		t.Errorf("expected final status 'failed', got '%s'", finalState.Status)
	}
	if finalState.StepResults["gate-check"].Status != "rejected" {
		t.Errorf("expected gate-check status 'rejected', got '%s'", finalState.StepResults["gate-check"].Status)
	}
}

func TestDAGTopologicalSort(t *testing.T) {
	steps := []StepDefinition{
		{ID: "step-c", Type: StepTypeCommand, Command: "echo C", DependsOn: []string{"step-b"}},
		{ID: "step-a", Type: StepTypeCommand, Command: "echo A"},
		{ID: "step-b", Type: StepTypeCommand, Command: "echo B", DependsOn: []string{"step-a"}},
	}
	sorted, err := TopologicalSortSteps(steps)
	if err != nil {
		t.Fatalf("unexpected sort error: %v", err)
	}
	expected := []string{"step-a", "step-b", "step-c"}
	for i, s := range sorted {
		if s.ID != expected[i] {
			t.Errorf("at index %d: expected %s, got %s", i, expected[i], s.ID)
		}
	}
}

func TestWorkflowCycleRejection(t *testing.T) {
	steps := []StepDefinition{
		{ID: "step-1", Type: StepTypeCommand, Command: "echo 1", DependsOn: []string{"step-2"}},
		{ID: "step-2", Type: StepTypeCommand, Command: "echo 2", DependsOn: []string{"step-1"}},
	}
	_, err := TopologicalSortSteps(steps)
	if err == nil {
		t.Fatalf("expected cycle rejection error, got nil")
	}
	if !strings.Contains(err.Error(), "cycle") {
		t.Errorf("expected error mentioning cycle, got: %v", err)
	}
}
