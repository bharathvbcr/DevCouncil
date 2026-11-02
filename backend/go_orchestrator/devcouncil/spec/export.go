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

		scopeEnv := map[string]any{
			"$schema":         "https://devcouncil.dev/schemas/scope-envelope.v1.json",
			"task_id":         st.ID,
			"user_story_id":   st.UserStoryRef,
			"requirement_ids": []string{featureID},
			"planned_files":   plannedFiles,
			"allowed_commands": []string{
				"go test ./...",
				"cargo test",
			},
			"expected_tests":  expectedTests,
			"parallel_safe":   st.ParallelSafe,
			"max_scope_bytes": 65536,
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

	// If not dry-run, persist exported tasks to .devcouncil/workbench/ or state store
	if !dryRun && projectRoot != "" {
		outDir := filepath.Join(projectRoot, ".devcouncil", "workbench", "tasks")
		_ = os.MkdirAll(outDir, 0755)
		for _, t := range res.Tasks {
			taskPath := filepath.Join(outDir, t.TaskID+".json")
			data, err := json.MarshalIndent(t, "", "  ")
			if err == nil {
				_ = os.WriteFile(taskPath, data, 0644)
			}
		}
	}

	return res, nil
}
