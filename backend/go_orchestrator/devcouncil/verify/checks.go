package verify

import (
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/policy"
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

// DetectOrphanDiffGaps flags files changed outside the editable planned set,
// and emits hard-safety `oracle_tampering` gaps if any `read_only` planned file
// was modified.
func DetectOrphanDiffGaps(taskID string, planned []dc.PlannedFile, changed []string, orphanAdded map[string]struct{}) []Gap {
	editableSet := make(map[string]struct{}, len(planned))
	readOnlySet := make(map[string]struct{})
	for _, pf := range planned {
		if pf.AllowedChange == dc.ChangeReadOnly {
			readOnlySet[pf.Path] = struct{}{}
			delete(editableSet, pf.Path)
			continue
		}
		if _, ro := readOnlySet[pf.Path]; !ro {
			editableSet[pf.Path] = struct{}{}
		}
	}
	var gaps []Gap
	for _, cf := range changed {
		if policy.IsToolState(cf) {
			continue
		}
		if _, ro := readOnlySet[cf]; ro {
			gaps = append(gaps, Gap{
				ID:             StableGapID(taskID, "READONLY-"+cf),
				Severity:       "critical",
				GapType:        "oracle_tampering",
				TaskID:         taskID,
				Description:    "Read-only planned file " + cf + " was modified during task execution.",
				Evidence:       []string{cf + " (allowed_change: read_only)"},
				RecommendedFix: "Revert unauthorized edits to read-only file " + cf + ", then re-verify.",
				Blocking:       true,
				File:           filePtr(cf),
			})
			continue
		}
		if _, ok := editableSet[cf]; ok {
			continue
		}
		_, isAdded := orphanAdded[cf]
		newTest := isAdded && IsAdvisoryNewTestPath(cf)
		if newTest {
			gaps = append(gaps, Gap{
				ID:       StableGapID(taskID, "ORPHAN-"+cf),
				Severity: "medium",
				GapType:  "orphan_diff",
				TaskID:   taskID,
				Description: "New test file " + cf + " was added but not planned for this task " +
					"(advisory: added tests cannot change shipped behavior).",
				Evidence: []string{cf},
				RecommendedFix: "Fold the tests into a planned test file, or ask the task's owner to add " + cf +
					" to its planned files.",
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
			RecommendedFix: "Revert changes to " + cf + ", or ask the task's owner to add it to the " +
				"task's planned files.",
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
		if pf.AllowedChange != dc.ChangeReadOnly {
			plannedSet[pf.Path] = struct{}{}
		}
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

// IsTestPath is isTestPath for callers outside the package: the preflight asks
// it to say whether an unplanned new file matches the shared test-path fixture.
func IsTestPath(path string) bool { return isTestPath(path) }

// IsRunnerBootstrapPath reports whether a path is a test-runner bootstrap,
// plugin, startup hook, or build/harness configuration file whose creation or
// modification can alter test discovery, exit status, or compilation globally
// (e.g. `tests/conftest.py`, `sitecustomize.py`, `*.pth`, `pytest.ini`,
// `build.rs`, `vitest.config.ts`). Unplanned creation inside a test directory
// must NEVER be downgraded to advisory even when IsTestPath(path) is true.
func IsRunnerBootstrapPath(path string) bool {
	norm := strings.ToLower(strings.ReplaceAll(path, "\\", "/"))
	norm = strings.TrimPrefix(norm, "./")
	if norm == ".cargo/config" || norm == ".cargo/config.toml" || strings.HasSuffix(norm, "/.cargo/config") || strings.HasSuffix(norm, "/.cargo/config.toml") {
		return true
	}
	base := norm
	if i := strings.LastIndexByte(norm, '/'); i >= 0 {
		base = norm[i+1:]
	}
	switch base {
	case "conftest.py", "sitecustomize.py", "usercustomize.py",
		"pytest.ini", ".pytest.ini", "tox.ini", "setup.cfg", "pyproject.toml",
		"build.rs", "cargo.toml", "package.json", "go.mod", ".gitattributes":
		return true
	}
	if strings.HasSuffix(base, ".pth") {
		return true
	}
	for _, prefix := range []string{
		"jest.config.", "vitest.config.", "playwright.config.", ".mocharc.", "karma.conf.",
	} {
		if strings.HasPrefix(base, prefix) {
			return true
		}
	}
	return false
}

// IsAdvisoryNewTestPath reports whether an unplanned newly added path may be
// treated as an advisory test addition rather than a blocking orphan. It holds
// isTestPath to protocols/test-paths/v1.json while excluding runner bootstrap
// files (`tests/conftest.py`, `tests/sitecustomize.py`, `tests/*.pth`, etc.)
// that execute hooks across the entire suite.
func IsAdvisoryNewTestPath(path string) bool {
	return isTestPath(path) && !IsRunnerBootstrapPath(path)
}

// isTestPath reports whether a path names a test file. It is the Go reading of
// the one rule protocols/test-paths/v1.json pins (devmap-query's is_test_path):
// a directory that is exactly a test directory, or a file carrying a test
// affix. The affixes are not bare substrings — `contest` and `testing_utils`
// are not tests — and the camel-case arm reads the original casing, so
// FooTest.java is a test and Latest.java is not. An unplanned *.spec.ts,
// spec/ or e2e/ file is therefore the advisory orphan gap an unplanned test
// file is, not a blocking one.
func isTestPath(path string) bool {
	norm := strings.ReplaceAll(path, "\\", "/")
	dir, name := "", norm
	if i := strings.LastIndexByte(norm, '/'); i >= 0 {
		dir, name = norm[:i], norm[i+1:]
		for _, segment := range strings.Split(dir, "/") {
			switch strings.ToLower(segment) {
			case "test", "tests", "spec", "specs", "__tests__", "testing", "e2e", "testdata":
				return true
			}
		}
	}
	return isTestFileName(name)
}

func isTestFileName(name string) bool {
	lowered := strings.ToLower(name)
	stem, _, _ := strings.Cut(name, ".")
	loweredStem, _, _ := strings.Cut(lowered, ".")
	for _, prefix := range []string{"test_", "spec_"} {
		if strings.HasPrefix(lowered, prefix) {
			return true
		}
	}
	for _, infix := range []string{".test.", ".spec.", "_test.", "_spec.", "-test.", "-spec."} {
		if strings.Contains(lowered, infix) {
			return true
		}
	}
	switch loweredStem {
	case "test", "tests", "spec", "specs":
		return true
	}
	for _, suffix := range []string{"Test", "Tests", "Spec", "Specs"} {
		if strings.HasSuffix(stem, suffix) {
			return true
		}
	}
	return false
}
