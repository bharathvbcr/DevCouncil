package proc

import (
	"context"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestGitSessionRunCapsOutputAndReportsTruncation(t *testing.T) {
	dir := gitRepo(t)
	if err := os.WriteFile(filepath.Join(dir, strings.Repeat("a", 200)+".txt"), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	s, err := NewGitSession(dir)
	if err != nil {
		t.Fatal(err)
	}
	res, err := s.Run(context.Background(), 64, "ls-files", "--others", "-z")
	if err != nil {
		t.Fatal(err)
	}
	if !res.Truncated || len(res.Stdout) != 64 {
		t.Fatalf("truncated=%v len=%d, want a 64-byte capped read reported as truncated", res.Truncated, len(res.Stdout))
	}
	res, err = s.Run(context.Background(), 1<<20, "ls-files", "--others", "-z")
	if err != nil || res.Truncated {
		t.Fatalf("uncapped read: err=%v truncated=%v", err, res.Truncated)
	}
}

func TestGitSessionRunCarriesStderrAndExitCode(t *testing.T) {
	dir := gitRepo(t)
	s, err := NewGitSession(dir)
	if err != nil {
		t.Fatal(err)
	}
	res, err := s.Run(context.Background(), 1<<10, "rev-parse", "--verify", "no-such-ref^{commit}")
	var exit *exec.ExitError
	if !errors.As(err, &exit) || res.ExitCode == 0 {
		t.Fatalf("want an *exec.ExitError and a non-zero code, got err=%v code=%d", err, res.ExitCode)
	}
	if !strings.Contains(err.Error(), "no-such-ref") {
		t.Fatalf("git's stderr is missing from the error: %v", err)
	}
}

func TestGitSessionRunHonoursCancellation(t *testing.T) {
	dir := gitRepo(t)
	s, err := NewGitSession(dir)
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	start := time.Now()
	if _, err := s.Run(ctx, 1<<10, "status"); err == nil {
		t.Fatal("a cancelled context must not report success")
	}
	if time.Since(start) > 10*time.Second {
		t.Fatal("cancelled read was not bounded")
	}
}
