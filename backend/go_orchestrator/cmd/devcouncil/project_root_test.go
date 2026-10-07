package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// rootCommands are the commands that resolve a project root, each in a form
// that would otherwise go on to read or write under it.
var rootCommands = [][]string{
	{"mcp"},
	{"gate", "status", "--json"},
	{"gate", "set", "--mode", "off"},
	{"skills", "scaffold", "--dry-run"},
	{"integrate", "claude", "--check"},
	{"integrate", "uninstall", "--check"},
	{"hook", "status"},
	{"verify", "TASK-1"},
	{"grep", "needle"},
}

// assertEmpty fails if anything was created in dir.
func assertEmpty(t *testing.T, dir, what string) {
	t.Helper()
	entries, err := os.ReadDir(dir)
	if err != nil {
		t.Fatal(err)
	}
	for _, e := range entries {
		t.Errorf("%s created %q in the working directory", what, e.Name())
	}
}

// A host that writes `"${CLAUDE_PROJECT_DIR}"` into a config without expanding
// it used to hand this binary that string as the project root. It was returned
// verbatim, so state went under a directory literally named ${CLAUDE_PROJECT_DIR}
// beside wherever the host started — the stray directory at this repository's
// root. Every command that resolves a root must refuse instead, and create
// nothing.
func TestUnexpandedProjectRootEnvIsRefused(t *testing.T) {
	cwd := t.TempDir()
	t.Chdir(cwd)
	t.Setenv("DEVCOUNCIL_PROJECT_ROOT", "${CLAUDE_PROJECT_DIR}")
	for _, args := range rootCommands {
		what := strings.Join(args, " ")
		if code := dispatch(args); code != 2 {
			t.Errorf("%s with an unexpanded DEVCOUNCIL_PROJECT_ROOT exited %d, want 2", what, code)
		}
		assertEmpty(t, cwd, what)
	}
	if _, err := os.Stat(filepath.Join(cwd, "${CLAUDE_PROJECT_DIR}")); !os.IsNotExist(err) {
		t.Fatalf("a directory named ${CLAUDE_PROJECT_DIR} exists: %v", err)
	}
}

// The same refusal through every --project-root parser, for each shape of root
// that cannot be the repository the caller meant.
func TestBadProjectRootFlagIsRefused(t *testing.T) {
	cwd := t.TempDir()
	t.Chdir(cwd)
	t.Setenv("DEVCOUNCIL_PROJECT_ROOT", cwd)
	file := filepath.Join(t.TempDir(), "not-a-dir")
	if err := os.WriteFile(file, nil, 0o644); err != nil {
		t.Fatal(err)
	}
	bad := map[string]string{
		"unexpanded braces": "${CLAUDE_PROJECT_DIR}",
		"unexpanded bare":   "$HOME/repo",
		"relative":          "repo",
		"missing":           filepath.Join(cwd, "does-not-exist"),
		"a file":            file,
		"empty":             "",
	}
	for _, args := range rootCommands {
		if args[0] == "mcp" {
			continue // takes no flags; its root is the environment's
		}
		for shape, root := range bad {
			argv := append(append([]string{}, args...), "--project-root", root)
			what := strings.Join(args, " ") + " --project-root " + shape
			if code := dispatch(argv); code != 2 {
				t.Errorf("%s exited %d, want 2", what, code)
			}
			assertEmpty(t, cwd, what)
		}
	}
}

func TestCheckProjectRoot(t *testing.T) {
	dir := t.TempDir()
	if got, err := checkProjectRoot(dir+string(filepath.Separator), "test"); err != nil || got != dir {
		t.Fatalf("checkProjectRoot(%q) = %q, %v; want %q", dir, got, err, dir)
	}
	for _, refused := range []string{"", "  ", "${X}", "$X", filepath.Join(dir, "$X"), "rel/path", filepath.Join(dir, "missing")} {
		if got, err := checkProjectRoot(refused, "test"); err == nil {
			t.Errorf("checkProjectRoot(%q) = %q, want a refusal", refused, got)
		}
	}
	// The refusal names its source, so the operator knows what to fix.
	_, err := checkProjectRoot("${CLAUDE_PROJECT_DIR}", "DEVCOUNCIL_PROJECT_ROOT")
	if err == nil || !strings.Contains(err.Error(), "DEVCOUNCIL_PROJECT_ROOT") || !strings.Contains(err.Error(), "unexpanded") {
		t.Fatalf("refusal = %v; want it to name DEVCOUNCIL_PROJECT_ROOT and the unexpanded variable", err)
	}
}
