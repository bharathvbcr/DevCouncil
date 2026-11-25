package prgateapp

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/gitrev"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/prgate"
)

// RunRequest carries the validated PR parameters handed to a gate Runner.
type RunRequest struct {
	Owner        string
	Repo         string
	CloneURL     string
	InstallToken string
	BaseSHA      string
	HeadSHA      string
	PullNumber   int
	Private      bool
}

// Runner executes `devcouncil pr-gate` against a pull request and returns the
// parsed `devcouncil.pr_gate/v1` report.
type Runner interface {
	Run(ctx context.Context, req RunRequest) (prgate.Report, error)
}

// CLIRunner shallow-clones the PR head and base commits into an ephemeral
// temporary directory (treating the checkout strictly as untrusted data) and
// runs the pinned external `devcouncil pr-gate` binary.
type CLIRunner struct {
	DevCouncilBin string
	DevMapBin     string
	DCVerifyBin   string
	DCRedactBin   string
	BuildIndex    bool
	Timeout       time.Duration
}

// Run performs an ephemeral shallow fetch of base and head commits (with hooks,
// fsmonitor, global/system gitconfig, and file:// protocol disabled, strict
// allowlisted subprocess environment, and credentials deleted immediately after
// `git fetch` before `git checkout` or `devcouncil pr-gate` runs).
func (r *CLIRunner) Run(ctx context.Context, req RunRequest) (prgate.Report, error) {
	if !gitrev.IsSHA40(req.BaseSHA) || !gitrev.IsSHA40(req.HeadSHA) {
		return prgate.Report{}, errors.New("invalid base or head commit SHA")
	}
	if strings.TrimSpace(r.DevCouncilBin) == "" {
		return prgate.Report{}, errors.New("devcouncil binary path is not configured")
	}
	if strings.TrimSpace(req.CloneURL) == "" {
		return prgate.Report{}, errors.New("repository clone URL is empty")
	}
	if !strings.HasPrefix(req.CloneURL, "https://") && !isLoopbackHTTP(req.CloneURL) {
		return prgate.Report{}, errors.New("refusing non-HTTPS repository clone URL")
	}

	timeout := r.Timeout
	if timeout <= 0 {
		timeout = 5 * time.Minute
	}
	runCtx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()

	workDir, err := os.MkdirTemp("", "devcouncil-app-run-*")
	if err != nil {
		return prgate.Report{}, fmt.Errorf("create ephemeral workdir: %w", err)
	}
	defer os.RemoveAll(workDir)

	repoDir := filepath.Join(workDir, "repo")
	homeDir := filepath.Join(workDir, "home")
	tmpSubDir := filepath.Join(workDir, "tmp")
	for _, d := range []string{repoDir, homeDir, tmpSubDir} {
		if err := os.MkdirAll(d, 0o700); err != nil {
			return prgate.Report{}, err
		}
	}

	// Build strict allowlist environment — never passes DEVCOUNCIL_WEBHOOK_SECRET,
	// DEVCOUNCIL_RELAY_TOKEN, private key paths, or ambient host credentials.
	baseEnv := allowlistedSubprocessEnv(homeDir, tmpSubDir)

	fetchEnv := append(append([]string(nil), baseEnv...),
		"GIT_TERMINAL_PROMPT=0",
		"GIT_CONFIG_NOSYSTEM=1",
		"GIT_CONFIG_GLOBAL=/dev/null",
	)

	var askpassPath, tokenPath string
	if strings.TrimSpace(req.InstallToken) != "" {
		askpassPath = filepath.Join(workDir, "askpass.sh")
		tokenPath = filepath.Join(workDir, "token")
		if err := os.WriteFile(tokenPath, []byte(req.InstallToken), 0o600); err != nil {
			return prgate.Report{}, err
		}
		script := fmt.Sprintf("#!/bin/sh\ncase \"$1\" in\n  *Username*) printf 'x-access-token\\n' ;;\n  *) cat %q ;;\nesac\n", tokenPath)
		if err := os.WriteFile(askpassPath, []byte(script), 0o700); err != nil {
			_ = os.Remove(tokenPath)
			return prgate.Report{}, err
		}
		fetchEnv = append(fetchEnv, "GIT_ASKPASS="+askpassPath)
	}
	purgeAuthFiles := func() {
		if tokenPath != "" {
			_ = os.Remove(tokenPath)
			tokenPath = ""
		}
		if askpassPath != "" {
			_ = os.Remove(askpassPath)
			askpassPath = ""
		}
	}
	defer purgeAuthFiles()

	gitRun := func(env []string, args ...string) (string, error) {
		hardened := append([]string{
			"-c", "core.hooksPath=/dev/null",
			"-c", "core.fsmonitor=false",
			"-c", "protocol.file.allow=never",
			"-c", "submodule.recurse=false",
		}, args...)
		cmd := exec.CommandContext(runCtx, "git", hardened...)
		cmd.Dir = repoDir
		cmd.Env = env
		var stdout, stderr bytes.Buffer
		cmd.Stdout = &stdout
		cmd.Stderr = &stderr
		if err := cmd.Run(); err != nil {
			return "", fmt.Errorf("git %s failed: %w", args[0], err)
		}
		return strings.TrimSpace(stdout.String()), nil
	}

	if _, err := gitRun(fetchEnv, "init", "--quiet"); err != nil {
		return prgate.Report{}, err
	}
	if _, err := gitRun(fetchEnv, "remote", "add", "origin", req.CloneURL); err != nil {
		return prgate.Report{}, err
	}
	if _, err := gitRun(fetchEnv, "fetch", "--no-tags", "--depth=1", "origin", req.BaseSHA, req.HeadSHA); err != nil {
		return prgate.Report{}, err
	}

	// CRITICAL SECURITY INVARIANT (C3): destroy the installation token and
	// askpass helper IMMEDIATELY after `git fetch` — before checking out
	// untrusted tree data or invoking `devcouncil pr-gate`.
	purgeAuthFiles()

	noAuthGitEnv := append(append([]string(nil), baseEnv...),
		"GIT_TERMINAL_PROMPT=0",
		"GIT_CONFIG_NOSYSTEM=1",
		"GIT_CONFIG_GLOBAL=/dev/null",
	)
	if _, err := gitRun(noAuthGitEnv, "checkout", "--detach", "--force", req.HeadSHA); err != nil {
		return prgate.Report{}, err
	}
	actualHead, err := gitRun(noAuthGitEnv, "rev-parse", "HEAD")
	if err != nil {
		return prgate.Report{}, err
	}
	if actualHead != req.HeadSHA {
		return prgate.Report{}, fmt.Errorf("checkout HEAD mismatch: got %s, expected %s", actualHead, req.HeadSHA)
	}

	// Ensure all pinned tool binaries live strictly outside the untrusted checkout.
	for _, bin := range []string{r.DevCouncilBin, r.DevMapBin, r.DCVerifyBin, r.DCRedactBin} {
		if bin == "" {
			continue
		}
		if err := ensurePathOutside(repoDir, bin); err != nil {
			return prgate.Report{}, err
		}
	}

	jsonOut := filepath.Join(workDir, "pr-gate.json")
	args := []string{
		"pr-gate",
		"--repo", repoDir,
		"--base", req.BaseSHA,
		"--head", req.HeadSHA,
		"--mode", "enforce",
		"--format", "json",
		"--json-out", jsonOut,
		fmt.Sprintf("--build-index=%t", r.BuildIndex),
		"--timeout", timeout.String(),
	}
	if r.DevMapBin != "" {
		args = append(args, "--devmap", r.DevMapBin)
	}
	if r.DCVerifyBin != "" {
		args = append(args, "--dcverify", r.DCVerifyBin)
	}
	if r.DCRedactBin != "" {
		args = append(args, "--dcredact", r.DCRedactBin)
	}

	cmd := exec.CommandContext(runCtx, r.DevCouncilBin, args...)
	cmd.Dir = workDir
	cmd.Env = append(append([]string(nil), baseEnv...), "GOTOOLCHAIN=local")
	var stdout, stderr bytes.Buffer
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr
	runErr := cmd.Run()

	// `devcouncil pr-gate` exits 1 on findings and 2 on degraded/error while
	// still emitting a valid JSON report to `--json-out`.
	reportBytes, readErr := os.ReadFile(jsonOut)
	if readErr != nil {
		if runErr != nil {
			return prgate.Report{}, fmt.Errorf("devcouncil pr-gate execution failed: %w", runErr)
		}
		return prgate.Report{}, fmt.Errorf("read pr-gate report: %w", readErr)
	}
	var report prgate.Report
	if err := json.Unmarshal(reportBytes, &report); err != nil {
		return prgate.Report{}, fmt.Errorf("decode pr-gate report: %w", err)
	}
	return report, nil
}

// allowlistedSubprocessEnv constructs a minimal, strict allowlist environment
// for git and `devcouncil pr-gate` children running against untrusted repos.
func allowlistedSubprocessEnv(homeDir, tmpDir string) []string {
	env := []string{
		"HOME=" + homeDir,
		"TMPDIR=" + tmpDir,
	}
	for _, key := range []string{"PATH", "LANG", "LC_ALL", "LC_CTYPE", "TZ"} {
		if v := os.Getenv(key); v != "" {
			env = append(env, key+"="+v)
		}
	}
	return env
}

func ensurePathOutside(root, candidate string) error {
	resolvedRoot, err := filepath.EvalSymlinks(root)
	if err != nil {
		resolvedRoot = filepath.Clean(root)
	}
	resolvedCand, err := filepath.EvalSymlinks(candidate)
	if err != nil {
		resolvedCand = filepath.Clean(candidate)
	}
	rel, err := filepath.Rel(resolvedRoot, resolvedCand)
	if err != nil {
		return nil
	}
	if rel == "." || (rel != ".." && !strings.HasPrefix(rel, ".."+string(os.PathSeparator))) {
		return fmt.Errorf("refusing binary %q inside untrusted repository checkout", candidate)
	}
	return nil
}
