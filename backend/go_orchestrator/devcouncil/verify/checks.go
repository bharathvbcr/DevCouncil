package verify

import (
	"path/filepath"
	"sort"
	"strings"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
)

var depFileNames = map[string]struct{}{
	"package.json": {}, "package-lock.json": {}, "yarn.lock": {}, "pnpm-lock.yaml": {},
	"requirements.txt": {}, "pyproject.toml": {}, "uv.lock": {}, "Pipfile.lock": {},
	"go.mod": {}, "go.sum": {}, "Cargo.toml": {}, "Cargo.lock": {},
}

// DetectNoWorkGap blocks when the task expects file changes but produced none.
func DetectNoWorkGap(taskID string, planned []dc.PlannedFile, workPresent bool) *Gap {
	expecting := plannedExpectingChange(planned)
	if workPresent || len(expecting) == 0 {
		return nil
	}
	sorted := append([]string(nil), expecting...)
	sort.Strings(sorted)
	return &Gap{
		ID:       StableGapID(taskID, "NODIFF"),
		Severity: "high",
		GapType:  "task_not_implemented",
		TaskID:   taskID,
		Description: "Task " + taskID + " declares files to create or modify, but produced no " +
			"changes. Verification cannot prove work that does not exist.",
		Evidence: []string{"planned files expecting change: " + PythonListRepr(sorted)},
		RecommendedFix: "Implement the planned changes so the diff is non-empty, then re-verify. " +
			"If you did make changes, ensure they are saved and visible to git " +
			"(not reverted, stashed, or written outside the project root).",
		Blocking: true,
	}
}

// DetectPlannedFileGaps returns advisory gaps for planned files not touched.
func DetectPlannedFileGaps(taskID string, planned []dc.PlannedFile, changed []string) []Gap {
	changedSet := make(map[string]struct{}, len(changed))
	for _, c := range changed {
		changedSet[c] = struct{}{}
	}
	var gaps []Gap
	for _, pf := range planned {
		if pf.AllowedChange == dc.ChangeReadOnly {
			continue
		}
		if _, ok := changedSet[pf.Path]; ok {
			continue
		}
		path := pf.Path
		gaps = append(gaps, Gap{
			ID:             StableGapID(taskID, "FILE-"+path),
			Severity:       "medium",
			GapType:        "planned_file_not_changed",
			TaskID:         taskID,
			Description:    "Planned file " + path + " was not modified.",
			Evidence:       []string{},
			RecommendedFix: "Modify " + path + " as planned or update the task.",
			Blocking:       false,
			File:           filePtr(path),
		})
	}
	return gaps
}

// DetectOrphanDiffGaps flags files changed outside the planned set.
func DetectOrphanDiffGaps(taskID string, planned []dc.PlannedFile, changed []string, orphanAdded map[string]struct{}) []Gap {
	plannedSet := make(map[string]struct{}, len(planned))
	for _, pf := range planned {
		plannedSet[pf.Path] = struct{}{}
	}
	var gaps []Gap
	for _, cf := range changed {
		if _, ok := plannedSet[cf]; ok {
			continue
		}
		_, isAdded := orphanAdded[cf]
		newTest := isAdded && isTestPath(cf)
		if newTest {
			gaps = append(gaps, Gap{
				ID:       StableGapID(taskID, "ORPHAN-"+cf),
				Severity: "medium",
				GapType:  "orphan_diff",
				TaskID:   taskID,
				Description: "New test file " + cf + " was added but not planned for this task " +
					"(advisory: added tests cannot change shipped behavior).",
				Evidence: []string{cf},
				RecommendedFix: "Append " + cf + " with `dev scope update <task_id> --lease-token <token> " +
					"--planned-file " + cf + "` (or fold the tests into a planned test file).",
				Blocking: false,
				File:     filePtr(cf),
			})
			continue
		}
		gaps = append(gaps, Gap{
			ID:          StableGapID(taskID, "ORPHAN-"+cf),
			Severity:    "high",
			GapType:     "orphan_diff",
			TaskID:      taskID,
			Description: "File " + cf + " was modified but not planned for this task.",
			Evidence:    []string{cf},
			RecommendedFix: "Revert changes to " + cf + " or append it with " +
				"`dev scope update <task_id> --lease-token <token> --planned-file " + cf + "`.",
			Blocking: true,
			File:     filePtr(cf),
		})
	}
	return gaps
}

// DetectDependencyRiskGaps blocks unplanned dependency-manifest edits.
func DetectDependencyRiskGaps(taskID string, planned []dc.PlannedFile, changed []string) []Gap {
	plannedSet := make(map[string]struct{}, len(planned))
	for _, pf := range planned {
		plannedSet[pf.Path] = struct{}{}
	}
	var gaps []Gap
	for _, path := range changed {
		if _, ok := depFileNames[filepath.Base(path)]; !ok {
			continue
		}
		if _, ok := plannedSet[path]; ok {
			continue
		}
		gaps = append(gaps, Gap{
			ID:             StableGapID(taskID, "DEP-"+path),
			Severity:       "high",
			GapType:        "dependency_risk",
			TaskID:         taskID,
			Description:    "Dependency file " + path + " was modified without being in planned files.",
			Evidence:       []string{path},
			RecommendedFix: "Justify the dependency change or revert " + path + ".",
			Blocking:       true,
			File:           filePtr(path),
		})
	}
	return gaps
}

func isTestPath(path string) bool {
	norm := strings.ReplaceAll(path, "\\", "/")
	base := filepath.Base(norm)
	if strings.HasPrefix(base, "test_") || strings.HasSuffix(base, "_test.go") ||
		strings.HasSuffix(base, "_test.py") || strings.HasSuffix(base, ".test.ts") ||
		strings.HasSuffix(base, ".test.js") || strings.HasSuffix(base, "_spec.rb") {
		return true
	}
	parts := strings.Split(norm, "/")
	for _, p := range parts {
		if p == "tests" || p == "test" || p == "__tests__" || p == "testdata" {
			return true
		}
	}
	return false
}
