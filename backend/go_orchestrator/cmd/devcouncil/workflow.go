package main

import (
	"strings"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/console"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/workflow"
)

const workflowHelp = `devcouncil workflow — declarative DAG workflow execution engine

Usage:
  devcouncil workflow run <path/to/workflow.yaml> [--var key=value] [--feature-id ID] [--project-root DIR]
  devcouncil workflow status <run_id> [--json] [--project-root DIR]
  devcouncil workflow resume <run_id> --decision approve|reject [--input key=value] [--project-root DIR]

Commands:
  run      Start a new workflow run from a YAML or JSON definition
  status   Display the current status and step outcomes of a workflow run
  resume   Resume a workflow paused at a gate with an explicit decision
`

func runWorkflow(args []string) int {
	if len(args) < 1 {
		console.Error(workflowHelp)
		return 2
	}

	switch args[0] {
	case "-h", "--help":
		console.Error(workflowHelp)
		return 0
	case "run":
		return runWorkflowRun(args[1:])
	case "status":
		return runWorkflowStatus(args[1:])
	case "resume":
		return runWorkflowResume(args[1:])
	default:
		console.Errorf("unknown workflow subcommand: %s\n", args[0])
		console.Error(workflowHelp)
		return 2
	}
}

func runWorkflowRun(args []string) int {
	root := ""
	workflowPath := ""
	featureID := ""
	vars := make(map[string]string)

	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "-h", "--help":
			console.Error(workflowHelp)
			return 0
		case "--feature-id":
			i++
			if i >= len(args) {
				console.Errorln("--feature-id requires a value")
				return 2
			}
			featureID = args[i]
		case "--var":
			i++
			if i >= len(args) {
				console.Errorln("--var requires key=value")
				return 2
			}
			parts := strings.SplitN(args[i], "=", 2)
			if len(parts) == 2 {
				vars[parts[0]] = parts[1]
			} else {
				console.Errorf("invalid --var format (expected key=value): %s\n", args[i])
				return 2
			}
		case "--project-root":
			i++
			r, ok := projectRootFlag(args, i)
			if !ok {
				return 2
			}
			root = r
		default:
			if workflowPath == "" && args[i][0] != '-' {
				workflowPath = args[i]
			} else {
				console.Errorf("unexpected argument: %s\n", args[i])
				return 2
			}
		}
	}

	if workflowPath == "" {
		console.Errorln("workflow run requires a workflow definition path (e.g. .devcouncil/workflows/feature-flow.yaml)")
		return 2
	}

	root, err := resolveRoot(root)
	if err != nil {
		console.Errorf("workflow run: %v\n", err)
		return 2
	}

	resolvedDefPath := workflow.WorkflowDefPath(root, workflowPath)
	def, err := workflow.ParseWorkflow(resolvedDefPath)
	if err != nil {
		console.Errorf("workflow run: failed to parse definition: %v\n", err)
		return 1
	}

	state, err := workflow.StartWorkflow(root, def, vars, featureID)
	if err != nil {
		console.Errorf("workflow execution failed: %v\n", err)
		if state != nil {
			console.Printf("Run ID: %s (status: %s)\n", state.RunID, state.Status)
		}
		return 1
	}

	console.Printf("Workflow %s (run %s) status: %s (current step: %s)\n",
		state.WorkflowName, state.RunID, state.Status, state.CurrentStep)
	if state.Status == "paused_at_gate" {
		gateRes := state.StepResults[state.CurrentStep]
		console.Printf("  Paused at gate: %s\n", state.CurrentStep)
		if gateRes.Prompt != "" {
			console.Printf("  Prompt: %s\n", gateRes.Prompt)
		}
		if len(gateRes.AllowedDecisions) > 0 {
			console.Printf("  Allowed decisions: %s\n", strings.Join(gateRes.AllowedDecisions, ", "))
		}
		console.Printf("  To resume: dev workflow resume %s --decision <choice>\n", state.RunID)
	}
	return 0
}

func runWorkflowStatus(args []string) int {
	root := ""
	runID := ""
	jsonOut := false

	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "-h", "--help":
			console.Error(workflowHelp)
			return 0
		case "--json":
			jsonOut = true
		case "--project-root":
			i++
			r, ok := projectRootFlag(args, i)
			if !ok {
				return 2
			}
			root = r
		default:
			if runID == "" && args[i][0] != '-' {
				runID = args[i]
			} else {
				console.Errorf("unexpected argument: %s\n", args[i])
				return 2
			}
		}
	}

	if runID == "" {
		console.Errorln("workflow status requires a run ID")
		return 2
	}

	root, err := resolveRoot(root)
	if err != nil {
		console.Errorf("workflow status: %v\n", err)
		return 2
	}

	state, err := workflow.LoadState(root, runID)
	if err != nil {
		console.Errorf("workflow status: %v\n", err)
		return 1
	}

	if jsonOut {
		if err := console.JSON(state); err != nil {
			console.Errorf("workflow status json: %v\n", err)
			return 1
		}
		return 0
	}

	console.Printf("Run ID:        %s\n", state.RunID)
	console.Printf("Workflow:      %s\n", state.WorkflowName)
	if state.FeatureID != "" {
		console.Printf("Feature ID:    %s\n", state.FeatureID)
	}
	console.Printf("Status:        %s\n", state.Status)
	console.Printf("Current Step:  %s\n", state.CurrentStep)
	console.Printf("Started:       %s\n", state.StartedAt)
	console.Printf("Updated:       %s\n", state.UpdatedAt)
	console.Println("\nStep Results:")
	for id, res := range state.StepResults {
		extra := ""
		if res.Decision != "" {
			extra = " [decision: " + res.Decision + "]"
		} else if res.ExitCode != 0 {
			extra = " [exit: " + string(rune(res.ExitCode+'0')) + "]"
		}
		console.Printf("  - %-20s: %s%s\n", id, res.Status, extra)
	}

	return 0
}

func runWorkflowResume(args []string) int {
	root := ""
	runID := ""
	decision := ""
	inputs := make(map[string]string)

	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "-h", "--help":
			console.Error(workflowHelp)
			return 0
		case "--decision":
			i++
			if i >= len(args) {
				console.Errorln("--decision requires a decision value (e.g. approve, reject)")
				return 2
			}
			decision = args[i]
		case "--input":
			i++
			if i >= len(args) {
				console.Errorln("--input requires key=value")
				return 2
			}
			parts := strings.SplitN(args[i], "=", 2)
			if len(parts) == 2 {
				inputs[parts[0]] = parts[1]
			}
		case "--project-root":
			i++
			r, ok := projectRootFlag(args, i)
			if !ok {
				return 2
			}
			root = r
		default:
			if runID == "" && args[i][0] != '-' {
				runID = args[i]
			} else {
				console.Errorf("unexpected argument: %s\n", args[i])
				return 2
			}
		}
	}

	if runID == "" {
		console.Errorln("workflow resume requires a run ID")
		return 2
	}
	if decision == "" {
		console.Errorln("workflow resume requires --decision (e.g. --decision approve)")
		return 2
	}

	root, err := resolveRoot(root)
	if err != nil {
		console.Errorf("workflow resume: %v\n", err)
		return 2
	}

	state, err := workflow.ResumeWorkflow(root, runID, decision, inputs)
	if err != nil {
		console.Errorf("workflow resume failed: %v\n", err)
		if state != nil {
			console.Printf("Status: %s\n", state.Status)
		}
		return 1
	}

	console.Printf("Workflow %s (run %s) resumed -> status: %s (current step: %s)\n",
		state.WorkflowName, state.RunID, state.Status, state.CurrentStep)
	return 0
}
