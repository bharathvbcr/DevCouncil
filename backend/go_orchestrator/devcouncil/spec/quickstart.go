package spec

import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
)

var reFeatureNum = regexp.MustCompile(`^(\d{3})[-_]`)

// NextFeatureNumber inspects specs/ to find the next available 3-digit sequential number.
func NextFeatureNumber(specsRoot string) int {
	entries, err := os.ReadDir(specsRoot)
	if err != nil {
		return 1
	}

	maxNum := 0
	for _, entry := range entries {
		if entry.IsDir() {
			if m := reFeatureNum.FindStringSubmatch(entry.Name()); len(m) > 1 {
				if num, err := strconv.Atoi(m[1]); err == nil && num > maxNum {
					maxNum = num
				}
			}
		}
	}
	return maxNum + 1
}

// ScaffoldFeature creates the 7-layer hierarchy under specs/###-<featureName>/.
func ScaffoldFeature(projectRoot, featureName string) (string, error) {
	cleanName := strings.ToLower(strings.TrimSpace(featureName))
	cleanName = strings.ReplaceAll(cleanName, " ", "-")
	cleanName = strings.ReplaceAll(cleanName, "_", "-")

	specsRoot := filepath.Join(projectRoot, "specs")
	if err := os.MkdirAll(specsRoot, 0755); err != nil {
		return "", fmt.Errorf("failed to create specs root: %w", err)
	}

	// If name already starts with 3 digits, use it directly; otherwise zero-pad
	var dirName string
	if reFeatureNum.MatchString(cleanName) {
		dirName = cleanName
	} else {
		nextNum := NextFeatureNumber(specsRoot)
		dirName = fmt.Sprintf("%03d-%s", nextNum, cleanName)
	}

	targetDir := filepath.Join(specsRoot, dirName)
	if fi, err := os.Stat(targetDir); err == nil && fi.IsDir() {
		return targetDir, fmt.Errorf("specification directory already exists: %s", targetDir)
	}

	if err := os.MkdirAll(filepath.Join(targetDir, "contracts"), 0755); err != nil {
		return "", fmt.Errorf("failed to create spec directory: %w", err)
	}

	// 1. spec.md
	specContent := fmt.Sprintf(`# Feature: %s

## 1. Problem Statement & Motivation
Describe the core problem this feature solves, user impact, and failure modes being prevented.

## 2. User Stories & Acceptance Scenarios

### US1: Primary Capability (Priority: P1 - MVP)
As a user or agent,
I want to execute the primary operation,
So that the objective is achieved with deterministic verification.

**Acceptance Scenarios**:
- **Scenario 1: Happy Path Execution**
  - **Given** valid configuration and prerequisites
  - **When** the operation is triggered
  - **Then** state transitions correctly and exits with code 0.

## 3. Success Metrics & Testability Criteria
- **SC-001**: Verification executes in under 150ms.
- **SC-002**: 100%% test coverage on new public interfaces.

## 4. Edge Cases & Exclusions
- **Exclusion**: Non-conforming payloads are rejected at the boundary.
`, dirName)

	// 2. research.md
	researchContent := `# Technical Research & Alternatives

## 1. Options Evaluated
- Evaluated existing internal libraries and crates.
- Considered external dependencies and confirmed adherence to Article VII (<= 3 modules).

## 2. Benchmarks & Memory Impact
- Confirmed zero ambient authority and allocator symmetry.
`

	// 3. plan.md
	planContent := fmt.Sprintf(`# Implementation Plan: %s

## 1. Technical Context
- Target Languages: Go / Rust
- Persistence: SQLite / dc-store

## 2. Constitution Check & Complexity Tracking
| Article | Status | Compliance Details / Justification |
| :--- | :--- | :--- |
| **I: Library-First** | PASSED | Implemented as standalone package |
| **II: CLI Consumer** | PASSED | CLI exposed with stdout/stderr separation |
| **III: Test-First** | PASSED | Tests written before implementation |
| **VII: Simplicity** | PASSED | 0 new external dependencies |

## 3. Complexity Tracking
*None. All proposed changes strictly adhere to Articles I through IX.*
`, dirName)

	// 4. data-model.md
	dataModelContent := `# Data Model

## 1. Entities & Types
Document core data structures, SQLite schemas, and serialization invariants.
`

	// 5. contracts/
	contractContent := `# Interface Contract
openapi: 3.1.0
info:
  title: Feature Interface Boundary
  version: 1.0.0
paths: {}
`

	// 6. tasks.md
	tasksContent := fmt.Sprintf(`# Tasks: %s

## Phase 1: Setup & Shared Infrastructure
- [ ] [T-001] Initialize data models and interface boundaries
- [ ] [T-002] [P] Scaffold unit and integration test fixtures

## Phase 2: Foundational Prerequisites
- [ ] [T-003] Implement core library logic

## Phase 3: User Story 1 - Primary Capability (P1 MVP)
- [ ] [T-004] [US1] Wire CLI commands and verify tests pass
`, dirName)

	// 7. quickstart.md
	quickstartContent := fmt.Sprintf(`# Quickstart: %s

## Verification Commands
Validate specification:
`+"```bash"+`
dev spec validate %s --json
`+"```"+`
`, dirName, dirName)

	files := map[string]string{
		"spec.md":                specContent,
		"research.md":            researchContent,
		"plan.md":                planContent,
		"data-model.md":          dataModelContent,
		"contracts/service.yaml": contractContent,
		"tasks.md":               tasksContent,
		"quickstart.md":          quickstartContent,
	}

	for relPath, content := range files {
		fullPath := filepath.Join(targetDir, relPath)
		if err := os.WriteFile(fullPath, []byte(content), 0644); err != nil {
			return "", fmt.Errorf("failed writing %s: %w", relPath, err)
		}
	}

	return targetDir, nil
}
