package main

import (
	"bytes"
	"encoding/json"
	"errors"
	"io/fs"
	"os"
	"path/filepath"
	"strings"
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

const committedGolden = "../../testdata/golden"

// copyGolden clones the corpus so a test can damage it.
func copyGolden(t *testing.T) string {
	t.Helper()
	dst := t.TempDir()
	err := filepath.WalkDir(committedGolden, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		rel, _ := filepath.Rel(committedGolden, path)
		if d.IsDir() {
			return os.MkdirAll(filepath.Join(dst, rel), 0o755)
		}
		b, err := os.ReadFile(path)
		if err != nil {
			return err
		}
		return os.WriteFile(filepath.Join(dst, rel), b, 0o644)
	})
	if err != nil {
		t.Fatal(err)
	}
	return dst
}

// This is the check CI runs. It used to fail on `skills count: want 17 got 18`
// because the count was a literal nobody was made to keep current.
func TestReplayPassesAgainstTheCommittedGolden(t *testing.T) {
	var out, errw bytes.Buffer
	if code := run(committedGolden, false, &out, &errw); code != 0 {
		t.Fatalf("exit %d\nstdout:\n%s\nstderr:\n%s", code, out.String(), errw.String())
	}
}

func TestReplayNamesWhatDriftedInTheSurface(t *testing.T) {
	golden := copyGolden(t)
	path := filepath.Join(golden, surfaceFile)
	raw, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var s surface
	if err := json.Unmarshal(raw, &s); err != nil {
		t.Fatal(err)
	}
	dropped := s.Skills[0]
	s.Skills = s.Skills[1:]
	s.Tools = append(s.Tools, "devcouncil_gone")
	edited, _ := json.Marshal(s)
	if err := os.WriteFile(path, edited, 0o644); err != nil {
		t.Fatal(err)
	}
	var out, errw bytes.Buffer
	if code := run(golden, false, &out, &errw); code != 1 {
		t.Fatalf("exit %d, want 1; stderr %s", code, errw.String())
	}
	for _, want := range []string{dropped, "devcouncil_gone"} {
		if !strings.Contains(errw.String(), want) {
			t.Fatalf("the failure must name %q:\n%s", want, errw.String())
		}
	}
}

func TestReplayFailsWhenAGetDiffEnvelopeDisagrees(t *testing.T) {
	golden := copyGolden(t)
	path := filepath.Join(golden, "mcp", "get_diff", "dirty.json")
	raw, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, bytes.Replace(raw, []byte(`"additions": 1`), []byte(`"additions": 9`), 1), 0o644); err != nil {
		t.Fatal(err)
	}
	var out, errw bytes.Buffer
	if code := run(golden, false, &out, &errw); code != 1 {
		t.Fatalf("exit %d, want 1; stderr %s", code, errw.String())
	}
}

func TestReplayRefusesAnEmptyOrMissingSurface(t *testing.T) {
	golden := copyGolden(t)
	path := filepath.Join(golden, surfaceFile)
	if err := os.WriteFile(path, []byte(`{"skills":[],"tools":[]}`), 0o644); err != nil {
		t.Fatal(err)
	}
	var out, errw bytes.Buffer
	if code := run(golden, false, &out, &errw); code != 1 {
		t.Fatalf("an empty golden surface must fail, exit %d", code)
	}
	if err := os.Remove(path); err != nil {
		t.Fatal(err)
	}
	if code := run(golden, false, &out, &errw); code != 1 {
		t.Fatalf("a missing golden surface must fail, exit %d", code)
	}
}

func TestReplayLeavesNoTemporaryRepositoryBehind(t *testing.T) {
	tmp := t.TempDir()
	t.Setenv("TMPDIR", tmp)
	var out, errw bytes.Buffer
	if code := run(committedGolden, false, &out, &errw); code != 0 {
		t.Fatalf("exit %d: %s", code, errw.String())
	}
	left, _ := filepath.Glob(filepath.Join(tmp, "dc-golden-*"))
	if len(left) != 0 {
		t.Fatalf("temporary directories left behind: %v", left)
	}
}
