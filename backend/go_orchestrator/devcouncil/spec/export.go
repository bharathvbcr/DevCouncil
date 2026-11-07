package spec

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

// ExportedTask represents a task transformed from tasks.md into a DevCouncil workbench task.
type ExportedTask struct {
	TaskID        string         `json:"task_id"`
	Title         string         `json:"title"`
	Status        string         `json:"status"`
	Priority      int            `json:"priority"`
	UserStoryRef  string         `json:"user_story_ref,omitempty"`
	ParallelSafe  bool           `json:"parallel_safe"`
	DependsOn     []string       `json:"depends_on,omitempty"`
	ScopeEnvelope map[string]any `json:"scope_envelope"`
}

// ExportResult contains the summary of an export-tasks run.
type ExportResult struct {
	FeatureID    string         `json:"feature_id"`
	SpecDir      string         `json:"spec_dir"`
	TargetStatus string         `json:"target_status"`
	DryRun       bool           `json:"dry_run"`
	TotalTasks   int            `json:"total_tasks"`
	Tasks        []ExportedTask `json:"tasks"`
}

// ExportTasks parses tasks.md in specDir and generates workbench tasks and scope envelopes.
func ExportTasks(projectRoot, specDir, targetStatus string, dryRun bool) (*ExportResult, error) {
	tasksFile := filepath.Join(specDir, "tasks.md")
	if !fileExists(tasksFile) {
		return nil, fmt.Errorf("tasks.md not found in %s", specDir)
	}

	specTasks, err := ParseTasksFile(tasksFile)
	if err != nil {
		return nil, fmt.Errorf("failed to parse tasks: %w", err)
	}

	if err := validateTaskDAG(specTasks); err != nil {
		return nil, err
	}

	if targetStatus == "" {
		targetStatus = "ready"
	}

	featureID := filepath.Base(specDir)
	res := &ExportResult{
		FeatureID:    featureID,
		SpecDir:      specDir,
		TargetStatus: targetStatus,
		DryRun:       dryRun,
		TotalTasks:   len(specTasks),
	}

	for _, st := range specTasks {
		// Priority based on UserStory or Phase
		priority := 2
		if strings.HasPrefix(strings.ToUpper(st.UserStoryRef), "US1") || st.Phase <= 2 {
			priority = 1
		} else if strings.HasPrefix(strings.ToUpper(st.UserStoryRef), "US3") {
			priority = 3
		}

		plannedFiles := st.TargetFiles
		if len(plannedFiles) == 0 {
			plannedFiles = []string{fmt.Sprintf("specs/%s/tasks.md", featureID)}
		}

		expectedTests := st.TestCommands
		if len(expectedTests) == 0 {
			expectedTests = []string{"go test ./..."}
		}

		// Canonical dc-store workbench scope schema (`rust/dc-store/src/workbench/scope.rs`).
		scopeEnv := map[string]any{
			"requirement_ids": []string{featureID},
			"planned_files":   plannedFiles,
			"allowed_commands": []string{
				"go test ./...",
				"cargo test",
			},
			"expected_tests": expectedTests,
		}

		expTask := ExportedTask{
			TaskID:        st.ID,
			Title:         st.Description,
			Status:        targetStatus,
			Priority:      priority,
			UserStoryRef:  st.UserStoryRef,
			ParallelSafe:  st.ParallelSafe,
			DependsOn:     st.DependsOn,
			ScopeEnvelope: scopeEnv,
		}

		res.Tasks = append(res.Tasks, expTask)
	}

	// If not dry-run, persist exported task envelopes to .devcouncil/workbench/tasks/.
	if !dryRun && projectRoot != "" {
		outDir := filepath.Join(projectRoot, ".devcouncil", "workbench", "tasks")
		if err := os.MkdirAll(outDir, 0o755); err != nil {
			return nil, fmt.Errorf("create workbench tasks dir: %w", err)
		}
		for _, t := range res.Tasks {
			taskPath := filepath.Join(outDir, t.TaskID+".json")
			data, err := json.MarshalIndent(t, "", "  ")
			if err != nil {
				return nil, fmt.Errorf("marshal task %s: %w", t.TaskID, err)
			}
			if err := os.WriteFile(taskPath, data, 0o644); err != nil {
				return nil, fmt.Errorf("write task %s: %w", t.TaskID, err)
			}
		}
	}

	return res, nil
}

func validateTaskDAG(tasks []SpecTask) error {
	depsByID := make(map[string][]string, len(tasks))
	for _, t := range tasks {
		if t.ID == "" {
			return fmt.Errorf("task has empty ID")
		}
		if _, dup := depsByID[t.ID]; dup {
			return fmt.Errorf("duplicate task ID %s in tasks.md", t.ID)
		}
		depsByID[t.ID] = t.DependsOn
	}
	for _, t := range tasks {
		for _, dep := range t.DependsOn {
			if dep == t.ID {
				return fmt.Errorf("task %s cannot depend on itself", t.ID)
			}
			if _, ok := depsByID[dep]; !ok {
				return fmt.Errorf("task %s depends on unknown task %s", t.ID, dep)
			}
		}
	}
	const (
		unvisited = 0
		visiting  = 1
		visited   = 2
	)
	state := make(map[string]int, len(tasks))
	var visit func(id string) error
	visit = func(id string) error {
		switch state[id] {
		case visiting:
			return fmt.Errorf("cyclic task dependency detected involving %s", id)
		case visited:
			return nil
		}
		state[id] = visiting
		for _, dep := range depsByID[id] {
			if err := visit(dep); err != nil {
				return err
			}
		}
		state[id] = visited
		return nil
	}
	for _, t := range tasks {
		if err := visit(t.ID); err != nil {
			return err
		}
	}
	return nil
}
