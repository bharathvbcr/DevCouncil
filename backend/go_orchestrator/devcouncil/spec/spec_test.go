package spec

import (
	"os"
	"path/filepath"
	"testing"
)

func TestParseAndValidateRealtimeSyncSpec(t *testing.T) {
	// Root of repo is 4 levels up from this package: devcouncil/spec -> devcouncil -> go_orchestrator -> backend -> DevCouncil
	wd, err := os.Getwd()
	if err != nil {
		t.Fatal(err)
	}

	repoRoot, err := filepath.Abs(filepath.Join(wd, "..", "..", "..", ".."))
	if err != nil {
		t.Fatal(err)
	}

	specDir, err := ResolveSpecDir(repoRoot, "001-realtime-sync")
	if err != nil {
		t.Fatalf("failed to resolve spec dir: %v", err)
	}

	parsed, err := ParseSpecification(specDir)
	if err != nil {
		t.Fatalf("failed to parse specification: %v", err)
	}

	if parsed.FeatureID != "001-realtime-sync" {
		t.Errorf("expected FeatureID 001-realtime-sync, got %s", parsed.FeatureID)
	}
	if len(parsed.UserStories) == 0 {
		t.Errorf("expected user stories, got 0")
	}
	if len(parsed.Tasks) == 0 {
		t.Errorf("expected tasks, got 0")
	}

	res, err := ValidateSpecification(parsed, true)
	if err != nil {
		t.Fatalf("validation returned error: %v", err)
	}
	if !res.Valid {
		t.Errorf("expected valid specification, got findings: %+v", res.Findings)
	}
}

func TestValidateAmbiguityDetection(t *testing.T) {
	tmpDir := t.TempDir()
	specFile := filepath.Join(tmpDir, "spec.md")
	content := `# Feature: Test Ambiguity

## 2. User Stories
### US1: Test Story (Priority: P1)
- **Scenario 1: Happy**
  - **Given** [NEEDS CLARIFICATION: exact timeout undefined]
  - **When** something happens
  - **Then** outcome is good
`
	if err := os.WriteFile(specFile, []byte(content), 0644); err != nil {
		t.Fatal(err)
	}

	spec := &Specification{
		FeatureID: "test-ambiguity",
		SpecPath:  specFile,
	}
	if err := parseSpecFile(spec); err != nil {
		t.Fatal(err)
	}

	if len(spec.Ambiguities) == 0 {
		t.Fatalf("expected ambiguities detected, got 0")
	}

	res, err := ValidateSpecification(spec, false)
	if err != nil {
		t.Fatal(err)
	}
	if res.Valid {
		t.Errorf("expected specification to fail validation due to [NEEDS CLARIFICATION], but it passed")
	}
}

func TestScaffoldFeature(t *testing.T) {
	tmpDir := t.TempDir()
	dir, err := ScaffoldFeature(tmpDir, "telemetry-stream")
	if err != nil {
		t.Fatalf("scaffold failed: %v", err)
	}

	expectedFiles := []string{
		"spec.md",
		"research.md",
		"plan.md",
		"data-model.md",
		"contracts/service.yaml",
		"tasks.md",
		"quickstart.md",
	}

	for _, f := range expectedFiles {
		p := filepath.Join(dir, f)
		if fi, err := os.Stat(p); err != nil || fi.IsDir() {
			t.Errorf("expected file %s does not exist", f)
		}
	}

	// Validate the scaffolded feature
	parsed, err := ParseSpecification(dir)
	if err != nil {
		t.Fatalf("failed to parse scaffolded spec: %v", err)
	}
	res, err := ValidateSpecification(parsed, false)
	if err != nil {
		t.Fatal(err)
	}
	if !res.Valid {
		t.Errorf("scaffolded feature should pass baseline validation, got: %+v", res.Findings)
	}
}

func TestExportTasks(t *testing.T) {
	tmpDir := t.TempDir()
	tasksMd := filepath.Join(tmpDir, "tasks.md")
	content := `# Execution Tasks: Feature 001
- [ ] TASK_01: Setup database schema migration depends_on: []
- [ ] TASK_02: Implement AST spec parser ` + "`backend/parser.go`" + ` depends_on: [TASK_01] tests: go test ./...
- [ ] [TASK_03] [P] [US1] Wire DevCouncil CLI export command depends_on: [TASK_01, TASK_02] tests: dev spec validate
`
	if err := os.WriteFile(tasksMd, []byte(content), 0644); err != nil {
		t.Fatal(err)
	}

	res, err := ExportTasks(tmpDir, tmpDir, "ready", true)
	if err != nil {
		t.Fatalf("export tasks failed: %v", err)
	}

	if res.TotalTasks != 3 {
		t.Fatalf("expected 3 tasks, got %d", res.TotalTasks)
	}

	t1 := res.Tasks[0]
	if t1.TaskID != "TASK_01" || len(t1.DependsOn) != 0 {
		t.Errorf("unexpected task 1: %+v", t1)
	}

	t2 := res.Tasks[1]
	if t2.TaskID != "TASK_02" || len(t2.DependsOn) != 1 || t2.DependsOn[0] != "TASK_01" {
		t.Errorf("unexpected task 2: %+v", t2)
	}

	t3 := res.Tasks[2]
	if t3.TaskID != "TASK_03" || !t3.ParallelSafe || t3.UserStoryRef != "US1" {
		t.Errorf("unexpected task 3: %+v", t3)
	}
	if len(t3.DependsOn) != 2 || t3.DependsOn[0] != "TASK_01" || t3.DependsOn[1] != "TASK_02" {
		t.Errorf("unexpected task 3 dependencies: %+v", t3.DependsOn)
	}
}
