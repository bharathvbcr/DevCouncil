package main

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// writeSpecFixture writes a specification under <root>/specs/<name> that
// passes strict validation; pass valid=false to drop its acceptance
// scenarios, which strict mode blocks on.
func writeSpecFixture(t *testing.T, root, name string, valid bool) {
	t.Helper()
	dir := filepath.Join(root, "specs", name)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	scenario := "- **Scenario 1: Happy**\n  - **Given** a configured repository\n  - **When** the gate runs\n  - **Then** it exits 0\n"
	if !valid {
		scenario = ""
	}
	files := map[string]string{
		"spec.md":  "# Feature: " + name + "\n\n## 2. User Stories\n### US1: Gate (Priority: P1)\n" + scenario,
		"plan.md":  "# Plan: " + name + "\n\n## Complexity Tracking\nNone.\n",
		"tasks.md": "# Tasks: " + name + "\n## Phase 1: Core\n- [ ] [T-001] [US1] Wire the gate\n",
	}
	for rel, content := range files {
		if err := os.WriteFile(filepath.Join(dir, rel), []byte(content), 0o644); err != nil {
			t.Fatal(err)
		}
	}
}

type specSuiteEnvelope struct {
	Valid        bool   `json:"valid"`
	Strict       bool   `json:"strict"`
	Total        int    `json:"total"`
	ValidCount   int    `json:"valid_count"`
	InvalidCount int    `json:"invalid_count"`
	Error        string `json:"error"`
	Specs        []struct {
		Valid     bool   `json:"valid"`
		FeatureID string `json:"feature_id"`
		Findings  []struct {
			Severity string `json:"severity"`
			Rule     string `json:"rule"`
		} `json:"findings"`
	} `json:"specs"`
}

func runSpecValidateCLI(t *testing.T, args ...string) (int, string, string) {
	t.Helper()
	stdout, restoreOut := swapStdout(t)
	stderr, restoreErr := swapStderr(t)
	code := dispatch(append([]string{"spec", "validate"}, args...))
	restoreOut()
	restoreErr()
	return code, stdout.String(), stderr.String()
}

func decodeSpecSuite(t *testing.T, stdout string) specSuiteEnvelope {
	t.Helper()
	var env specSuiteEnvelope
	if err := json.Unmarshal([]byte(stdout), &env); err != nil {
		t.Fatalf("spec validate --all --json printed no envelope: %v (%q)", err, stdout)
	}
	return env
}

func TestSpecValidateAllPassesEverySpecAndIgnoresNonSpecEntries(t *testing.T) {
	root := t.TempDir()
	writeSpecFixture(t, root, "001-alpha", true)
	writeSpecFixture(t, root, "002-beta", true)
	// Not NNN- spec directories: a Lean project, a README, an underscore id
	// and a two-digit id must all be left alone.
	for _, dir := range []string{"lean", "010_underscore", "99-short"} {
		if err := os.MkdirAll(filepath.Join(root, "specs", dir), 0o755); err != nil {
			t.Fatal(err)
		}
	}
	if err := os.WriteFile(filepath.Join(root, "specs", "README.md"), []byte("# Specs\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "specs", "003-a-file.md"), []byte("not a dir\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	code, stdout, stderr := runSpecValidateCLI(t, "--all", "--strict", "--json", "--project-root", root)
	if code != 0 {
		t.Fatalf("spec validate --all exit %d, want 0; stderr=%s stdout=%s", code, stderr, stdout)
	}
	env := decodeSpecSuite(t, stdout)
	if !env.Valid || !env.Strict || env.Total != 2 || env.ValidCount != 2 || env.InvalidCount != 0 {
		t.Fatalf("unexpected envelope: %+v", env)
	}
	if len(env.Specs) != 2 || env.Specs[0].FeatureID != "001-alpha" || env.Specs[1].FeatureID != "002-beta" {
		t.Fatalf("expected exactly 001-alpha and 002-beta in order, got %+v", env.Specs)
	}

	code, stdout, stderr = runSpecValidateCLI(t, "--all", "--strict", "--project-root", root)
	if code != 0 || !strings.Contains(stdout, "2 of 2") {
		t.Fatalf("text mode: exit %d stdout=%q stderr=%q", code, stdout, stderr)
	}
}

func TestSpecValidateAllFailsWhenOneSpecIsInvalid(t *testing.T) {
	root := t.TempDir()
	writeSpecFixture(t, root, "001-good", true)
	writeSpecFixture(t, root, "002-bad", false)

	code, stdout, stderr := runSpecValidateCLI(t, "--all", "--strict", "--json", "--project-root", root)
	if code != 1 {
		t.Fatalf("spec validate --all exit %d, want 1; stderr=%s stdout=%s", code, stderr, stdout)
	}
	env := decodeSpecSuite(t, stdout)
	if env.Valid || env.Total != 2 || env.ValidCount != 1 || env.InvalidCount != 1 {
		t.Fatalf("unexpected envelope: %+v", env)
	}
	if len(env.Specs) != 2 || env.Specs[1].FeatureID != "002-bad" || env.Specs[1].Valid {
		t.Fatalf("expected 002-bad reported invalid, got %+v", env.Specs)
	}

	// Without --strict the missing scenarios are advisory, so the same tree passes.
	if code, stdout, stderr := runSpecValidateCLI(t, "--all", "--json", "--project-root", root); code != 0 {
		t.Fatalf("non-strict --all exit %d, want 0; stderr=%s stdout=%s", code, stderr, stdout)
	}
}

func TestSpecValidateAllFailsClosedWhenNothingToCheck(t *testing.T) {
	missing := t.TempDir()
	code, stdout, stderr := runSpecValidateCLI(t, "--all", "--strict", "--json", "--project-root", missing)
	if code != 1 || !strings.Contains(stderr, "specs") {
		t.Fatalf("missing specs/: exit %d stderr=%q, want 1 naming specs/", code, stderr)
	}
	if env := decodeSpecSuite(t, stdout); env.Valid || env.Total != 0 || env.Error == "" {
		t.Fatalf("missing specs/ envelope must be invalid with an error: %+v", env)
	}

	empty := t.TempDir()
	if err := os.MkdirAll(filepath.Join(empty, "specs", "lean"), 0o755); err != nil {
		t.Fatal(err)
	}
	code, stdout, stderr = runSpecValidateCLI(t, "--all", "--strict", "--json", "--project-root", empty)
	if code != 1 || !strings.Contains(stderr, "no NNN-") {
		t.Fatalf("zero specs: exit %d stderr=%q, want 1 saying no NNN- spec directories", code, stderr)
	}
	if env := decodeSpecSuite(t, stdout); env.Valid || env.Total != 0 || env.Error == "" {
		t.Fatalf("zero specs envelope must be invalid with an error: %+v", env)
	}
}

func TestSpecValidateAllWithFeatureNameIsUsageError(t *testing.T) {
	root := t.TempDir()
	writeSpecFixture(t, root, "001-alpha", true)
	for _, args := range [][]string{
		{"--all", "001-alpha", "--project-root", root},
		{"001-alpha", "--all", "--project-root", root},
	} {
		code, stdout, stderr := runSpecValidateCLI(t, args...)
		if code != 2 || !strings.Contains(stderr, "--all") || !strings.Contains(stderr, "feature name") {
			t.Fatalf("args %v: exit %d stderr=%q stdout=%q, want 2 naming the --all/feature name conflict", args, code, stderr, stdout)
		}
	}
}

func TestSpecValidateAllStrictBlocksUneditedScaffold(t *testing.T) {
	root := t.TempDir()
	if code := dispatch([]string{"spec", "init", "telemetry", "--project-root", root}); code != 0 {
		t.Fatalf("spec init exit %d", code)
	}
	code, stdout, stderr := runSpecValidateCLI(t, "--all", "--strict", "--json", "--project-root", root)
	if code != 1 {
		t.Fatalf("scaffold-only tree: exit %d, want 1; stderr=%s stdout=%s", code, stderr, stdout)
	}
	env := decodeSpecSuite(t, stdout)
	if len(env.Specs) != 1 {
		t.Fatalf("expected one spec, got %+v", env)
	}
	found := false
	for _, f := range env.Specs[0].Findings {
		if f.Rule == "scaffold_placeholder" && f.Severity == "blocking" {
			found = true
		}
	}
	if !found {
		t.Fatalf("expected a blocking scaffold_placeholder finding, got %+v", env.Specs[0].Findings)
	}
}

func TestSpecValidateAllStrictPassesThisRepository(t *testing.T) {
	wd, err := os.Getwd()
	if err != nil {
		t.Fatal(err)
	}
	// backend/go_orchestrator/cmd/devcouncil -> repository root
	repoRoot, err := filepath.Abs(filepath.Join(wd, "..", "..", "..", ".."))
	if err != nil {
		t.Fatal(err)
	}
	code, stdout, stderr := runSpecValidateCLI(t, "--all", "--strict", "--json", "--project-root", repoRoot)
	if code != 0 {
		t.Fatalf("repository specs fail strict --all: exit %d stderr=%s stdout=%s", code, stderr, stdout)
	}
	if env := decodeSpecSuite(t, stdout); env.Total < 4 || env.InvalidCount != 0 {
		t.Fatalf("expected at least the 4 committed specs, all valid: %+v", env)
	}
}

func TestSpecExportTasksPersistsWorkbenchItemsWithScopeAndDAG(t *testing.T) {
	bin := testsupport.DCStore(t)
	t.Setenv("PATH", filepath.Dir(bin)+string(os.PathListSeparator)+os.Getenv("PATH"))

	root := t.TempDir()
	specDir := filepath.Join(root, "specs", "002-zero-copy-worktrees")
	if err := os.MkdirAll(specDir, 0o755); err != nil {
		t.Fatal(err)
	}
	longProse := "Implement bidirectional `same_registered_repo`, `--root` worktree support in `mcp::open_for_call`, asymmetric `RootResolveInput::resolve`, `PostToolUse` lazy seed in `hook.rs`, and guaranteed `--seed-from` manifest write across all stacked worktrees and warm MCP servers with strict zero-write-on-read invariants and atomic staging"
	tasksMd := "# Tasks: 002 - Zero-Copy\n## Phase 1: Core\n" +
		"- [x] [T-001] [US1] " + longProse + " `rust/devmap-extract/src/paths.rs` `rust/devmap-serve/src/mcp.rs` tests: cargo test -p devmap-serve\n" +
		"- [ ] [T-002] [P] [US2] Retire legacy overlay sidecars `src-tauri/src/codeintel/worktree_overlay.rs` depends_on: [T-001] tests: cargo test --lib worktree_overlay\n"
	if err := os.WriteFile(filepath.Join(specDir, "tasks.md"), []byte(tasksMd), 0o644); err != nil {
		t.Fatal(err)
	}

	stdout, restoreOut := swapStdout(t)
	stderr, restoreErr := swapStderr(t)
	code := dispatch([]string{"spec", "export-tasks", "002-zero-copy-worktrees", "--json", "--project-root", root})
	restoreOut()
	restoreErr()
	if code != 0 {
		t.Fatalf("spec export-tasks exit %d stderr=%s stdout=%s", code, stderr.String(), stdout.String())
	}

	// Verify `devcouncil task show T-001 --json --project-root <root>` succeeds against `.devcouncil/state.sqlite`!
	showOut, restoreShowOut := swapStdout(t)
	showErr, restoreShowErr := swapStderr(t)
	showCode := dispatch([]string{"task", "show", "T-001", "--json", "--project-root", root})
	restoreShowOut()
	restoreShowErr()
	if showCode != 0 {
		t.Fatalf("task show T-001 exit %d stderr=%s stdout=%s", showCode, showErr.String(), showOut.String())
	}

	var payload struct {
		OK   bool `json:"ok"`
		Item struct {
			ID    string `json:"id"`
			Links []struct {
				Kind   string `json:"kind"`
				ItemID string `json:"item_id"`
			} `json:"links"`
			Scope struct {
				PlannedFiles []any `json:"planned_files"`
			} `json:"scope"`
		} `json:"item"`
	}
	if err := json.Unmarshal(showOut.Bytes(), &payload); err != nil {
		t.Fatalf("unmarshal task show output: %v (%s)", err, showOut.String())
	}
	if !payload.OK || payload.Item.ID != "T-001" {
		t.Fatalf("unexpected task show payload: %s", showOut.String())
	}
	if len(payload.Item.Scope.PlannedFiles) != 2 {
		t.Fatalf("expected 2 planned_files on T-001, got %v", payload.Item.Scope.PlannedFiles)
	}
	if len(payload.Item.Links) != 1 || payload.Item.Links[0].Kind != "blocks" || payload.Item.Links[0].ItemID != "T-002" {
		t.Fatalf("expected T-001 to carry blocks link to T-002, got %+v", payload.Item.Links)
	}

	// Re-running `spec export-tasks` must be idempotent!
	out2, rOut2 := swapStdout(t)
	err2, rErr2 := swapStderr(t)
	code2 := dispatch([]string{"spec", "export-tasks", "002-zero-copy-worktrees", "--json", "--project-root", root})
	rOut2()
	rErr2()
	if code2 != 0 {
		t.Fatalf("idempotent re-export exit %d stderr=%s stdout=%s", code2, err2.String(), out2.String())
	}
}
