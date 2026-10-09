package devcouncil_test

import (
	"context"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil"
)

// A git the repository carries does not get to say what the repository
// contains. One that an absolute PATH entry places inside the checkout (as
// direnv's PATH_add does) is refused, and the refusal reads as "could not run
// git", not as "this is not a repository".
func TestRepositoryLocalGitIsNotRun(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("the planted git is a shell script")
	}
	root := t.TempDir()
	marker := filepath.Join(t.TempDir(), "ran")
	if err := os.MkdirAll(filepath.Join(root, "bin"), 0o755); err != nil {
		t.Fatal(err)
	}
	script := "#!/bin/sh\n: > \"$PLANTED_GIT_MARKER\"\necho true\n"
	// #nosec G306 -- an executable stand-in; the execute bit is the point.
	if err := os.WriteFile(filepath.Join(root, "bin", "git"), []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PLANTED_GIT_MARKER", marker)
	t.Setenv("PATH", filepath.Join(root, "bin"))

	state := devcouncil.ProbeGitRepoState(context.Background(), root)
	if _, err := os.Stat(marker); err == nil {
		t.Fatal("the repository's git ran")
	}
	if state.InRepo != nil {
		t.Fatalf("a refused git produced a repository answer: %+v", state)
	}
	if !strings.Contains(state.Reason, "inside the repository") {
		t.Fatalf("reason does not name the refusal: %q", state.Reason)
	}
}
