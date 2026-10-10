package main

import (
	"errors"
	"os"
	"path/filepath"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
)

// The replay seeds its repository with git init, config and commit, run from
// the module directory inside this checkout. A git the checkout carries on
// PATH is refused and never started, so building the seed repository fails
// instead of running it.
func TestSeedRepositoryNeverRunsAGitInsideTheCheckout(t *testing.T) {
	checkout := t.TempDir()
	module := filepath.Join(checkout, "backend", "go_orchestrator")
	if err := os.MkdirAll(module, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.Mkdir(filepath.Join(checkout, ".git"), 0o755); err != nil {
		t.Fatal(err)
	}
	marker := filepath.Join(t.TempDir(), "planted-git-ran")
	bin := filepath.Join(checkout, "tools", "bin")
	if err := os.MkdirAll(bin, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(bin, "git"), []byte("#!/bin/sh\n: > '"+marker+"'\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", bin)
	t.Chdir(module)

	dir, err := seedRepo()
	if dir != "" {
		t.Cleanup(func() { os.RemoveAll(dir) })
	}
	if _, statErr := os.Stat(marker); statErr == nil {
		t.Fatalf("the checkout's git ran (err %v)", err)
	}
	if !errors.Is(err, proc.ErrRepositoryLocal) {
		t.Fatalf("err = %v, want proc.ErrRepositoryLocal", err)
	}
}
