package verify_test

import (
	"bytes"
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/gating"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
)

func hasLiveProcessWithToken(token string) bool {
	entries, err := os.ReadDir("/proc")
	if err != nil {
		return false
	}
	for _, de := range entries {
		if !de.IsDir() {
			continue
		}
		if _, err := strconv.Atoi(de.Name()); err != nil {
			continue
		}
		pidDir := filepath.Join("/proc", de.Name())
		cmdline, err := os.ReadFile(filepath.Join(pidDir, "cmdline"))
		if err != nil || !bytes.Contains(cmdline, []byte(token)) {
			continue
		}
		statBytes, err := os.ReadFile(filepath.Join(pidDir, "stat"))
		if err == nil {
			// Ignore already-terminated zombie entries (`) Z `).
			if idx := bytes.LastIndexByte(statBytes, ')'); idx >= 0 && idx+2 < len(statBytes) {
				if statBytes[idx+2] == 'Z' {
					continue
				}
			}
		}
		return true
	}
	return false
}

// TestSandboxUnavailableEmitsHardSafetyOracleGap verifies cross-platform that
// whenever a read-only verification sandbox is unavailable or blocked from
// creating namespaces, verify.Run emits a non-demotable blocking
// ORACLE-SANDBOX-UNAVAILABLE gap (`rigor_check_unavailable`) and never executes
// the command unsandboxed on the host.
func TestSandboxUnavailableEmitsHardSafetyOracleGap(t *testing.T) {
	ctx := context.Background()
	repo := t.TempDir()
	canary := filepath.Join(repo, "canary.txt")
	if err := os.WriteFile(canary, []byte("SAFE"), 0o644); err != nil {
		t.Fatal(err)
	}
	impl := filepath.Join(repo, "impl.go")
	if err := os.WriteFile(impl, []byte("package p\nfunc Add(a, b int) int { return a + b }\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	blockedBwrap := filepath.Join(t.TempDir(), "blocked-bwrap.sh")
	if err := os.WriteFile(blockedBwrap, []byte("#!/bin/sh\necho 'bwrap: setting up uid map: Permission denied' >&2\nexit 1\n"), 0o755); err != nil {
		t.Fatal(err)
	}

	task := &store.Task{
		ID:         "T-SBX-DENIED",
		Difficulty: "medium",
		PlannedFiles: []dc.PlannedFile{
			{Path: "impl.go", AllowedChange: dc.ChangeModify},
		},
		ExpectedTests: []string{"echo PWNED > canary.txt"},
	}

	gaps, _ := verify.Run(ctx, verify.Input{
		Root:         repo,
		Task:         task,
		GateMode:     "warn",
		AllowNoRigor: true,
		WorkPresent:  true,
		ChangedFiles: []string{"impl.go"},
		Sandbox:      verify.SandboxBwrapRO,
		RunCommand: func(cmd string) verify.CommandOutcome {
			return verify.RunLinuxBwrapVerificationCommandWithBin(ctx, blockedBwrap, repo, cmd, 5*time.Second, verify.EnvPolicy{})
		},
	})

	foundGap := false
	for _, g := range gaps {
		if strings.Contains(g.Description, "ORACLE-SANDBOX-UNAVAILABLE") {
			foundGap = true
			if g.GapType != "rigor_check_unavailable" || !g.Blocking {
				t.Fatalf("expected blocking rigor_check_unavailable gap, got %+v", g)
			}
			if gating.MayDemote(g.GapType, g.Blocking) {
				t.Fatalf("ORACLE-SANDBOX-UNAVAILABLE gap type %q must never be demotable in warn mode", g.GapType)
			}
		}
	}
	if !foundGap {
		t.Fatalf("expected ORACLE-SANDBOX-UNAVAILABLE gap when bwrap namespace setup is denied, got %+v", gaps)
	}

	gotCanary, err := os.ReadFile(canary)
	if err != nil || string(gotCanary) != "SAFE" {
		t.Fatalf("sandbox denial fallback executed command unsandboxed! canary=%q err=%v", string(gotCanary), err)
	}
}

// TestLinuxKernelBwrapRuntimeEnforcement exercises live Linux kernel `bwrap`
// user/mount namespaces when running on Linux:
//   - When namespaces are denied (e.g. default unprivileged container or Ubuntu
//     AppArmor userns restriction) and `REQUIRE_BWRAP_SANDBOX != "1"`, asserts
//     `ProbeLinuxBwrapWithBin`, `RunSandboxedVerificationCommand`, and `verify.Run`
//     fail closed (`ORACLE-SANDBOX-UNAVAILABLE`) without running unsandboxed.
//   - When `REQUIRE_BWRAP_SANDBOX=1` (or when `ProbeLinuxBwrapWithBin` succeeds),
//     asserts live `bwrap` creates user/mount namespaces, blocks writes to repo
//     files with `EROFS`, allows isolated `TMPDIR`/`GOCACHE`/`XDG_CACHE_HOME`
//     writes, and reaps child process groups on timeout.
func TestLinuxKernelBwrapRuntimeEnforcement(t *testing.T) {
	if runtime.GOOS != "linux" {
		t.Skip("live Linux kernel bwrap integration test runs only on linux")
	}

	requireBwrap := strings.TrimSpace(os.Getenv("REQUIRE_BWRAP_SANDBOX")) == "1"
	expectDenied := strings.TrimSpace(os.Getenv("TEST_EXPECT_BWRAP_DENIED")) == "1"

	bwrapPath, lookErr := exec.LookPath("bwrap")
	if lookErr != nil {
		if requireBwrap || expectDenied {
			t.Fatalf("bwrap binary missing on Linux host while sandbox verification required: %v", lookErr)
		}
		t.Skipf("bwrap binary not installed on Linux host (set REQUIRE_BWRAP_SANDBOX=1 in CI): %v", lookErr)
	}

	ctx := context.Background()
	probeErr := verify.ProbeLinuxBwrapWithBin(ctx, bwrapPath)
	if probeErr != nil {
		if requireBwrap {
			t.Fatalf("REQUIRE_BWRAP_SANDBOX=1 set, but live Linux bwrap namespace probe failed (%s): %v", bwrapPath, probeErr)
		}

		// Mode A: Unprivileged container / AppArmor userns restriction — verify
		// real `/usr/bin/bwrap` failure triggers strict fail-closed refusal.
		repo := t.TempDir()
		canary := filepath.Join(repo, "canary.txt")
		if err := os.WriteFile(canary, []byte("SAFE"), 0o644); err != nil {
			t.Fatal(err)
		}
		if _, err := verify.ParseSandbox(verify.SandboxBwrapRO); err == nil {
			t.Fatal("ParseSandbox(SandboxBwrapRO) succeeded despite failing bwrap probe")
		}
		if _, err := verify.ParseSandbox(verify.SandboxReadOnly); err == nil {
			t.Fatal("ParseSandbox(SandboxReadOnly) succeeded despite failing bwrap probe")
		}

		out := verify.RunSandboxedVerificationCommand(
			ctx,
			repo,
			"echo PWNED > canary.txt",
			5*time.Second,
			verify.EnvPolicy{},
		)
		if !out.Skipped || !out.SandboxUnavailable || out.ExitCode != -1 {
			t.Fatalf("expected fail-closed Skipped+SandboxUnavailable outcome on denied namespaces, got %+v", out)
		}

		task := &store.Task{
			ID:            "T-LIVE-BWRAP-DENIED",
			Difficulty:    "medium",
			PlannedFiles:  []dc.PlannedFile{{Path: "canary.txt", AllowedChange: dc.ChangeReadOnly}},
			ExpectedTests: []string{"echo PWNED > canary.txt"},
		}
		gaps, _ := verify.Run(ctx, verify.Input{
			Root:         repo,
			Task:         task,
			Sandbox:      verify.SandboxReadOnly,
			GateMode:     "warn",
			AllowNoRigor: true,
			WorkPresent:  true,
		})
		sawSandboxGap := false
		for _, g := range gaps {
			if strings.Contains(g.Description, "ORACLE-SANDBOX-UNAVAILABLE") && g.Blocking && g.GapType == "rigor_check_unavailable" {
				sawSandboxGap = true
			}
		}
		if !sawSandboxGap {
			t.Fatalf("expected blocking ORACLE-SANDBOX-UNAVAILABLE gap in verify.Run when Linux namespaces are denied, got %+v", gaps)
		}
		gotCanary, err := os.ReadFile(canary)
		if err != nil || string(gotCanary) != "SAFE" {
			t.Fatalf("command executed unsandboxed when bwrap namespaces were denied! canary=%q err=%v", string(gotCanary), err)
		}
		return
	}

	if expectDenied {
		t.Fatalf("TEST_EXPECT_BWRAP_DENIED=1 was set, but ProbeLinuxBwrapWithBin(%q) succeeded", bwrapPath)
	}

	// Mode B: Namespace-capable Linux kernel — verify live bwrap mount isolation!
	if got, err := verify.ParseSandbox(verify.SandboxReadOnly); err != nil || got != verify.SandboxBwrapRO {
		t.Fatalf("ParseSandbox(SandboxReadOnly) = (%q, %v), want (%q, nil)", got, err, verify.SandboxBwrapRO)
	}

	repo := t.TempDir()
	oracleFile := filepath.Join(repo, "oracle_test.go")
	const oracleOriginal = "package p\n// HELD_OUT_ORACLE_V1\n"
	if err := os.WriteFile(oracleFile, []byte(oracleOriginal), 0o644); err != nil {
		t.Fatal(err)
	}
	dotGitDir := filepath.Join(repo, ".git")
	if err := os.MkdirAll(dotGitDir, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dotGitDir, "HEAD"), []byte("ref: refs/heads/main\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	// 1. Reachability + isolated scratch TMPDIR / GOCACHE / XDG_CACHE_HOME writes.
	okOut := verify.RunSandboxedVerificationCommand(
		ctx,
		repo,
		"grep -q HELD_OUT_ORACLE_V1 oracle_test.go && "+
			"echo t > \"$TMPDIR/t.ok\" && "+
			"echo g > \"$GOCACHE/g.ok\" && "+
			"echo x > \"$XDG_CACHE_HOME/x.ok\" && "+
			"printf 'LIVE_BWRAP_OK'",
		10*time.Second,
		verify.EnvPolicy{},
	)
	if okOut.ExitCode != 0 || !strings.Contains(okOut.Stdout, "LIVE_BWRAP_OK") {
		t.Fatalf("expected live Linux bwrap read + isolated cache writes to succeed, got %+v", okOut)
	}

	// 2. Kernel EROFS write denial on repository files and .git metadata.
	roFileOut := verify.RunSandboxedVerificationCommand(
		ctx,
		repo,
		"echo TAMPERED > oracle_test.go",
		10*time.Second,
		verify.EnvPolicy{},
	)
	if roFileOut.ExitCode == 0 {
		t.Fatalf("live Linux bwrap permitted writing to read-only repository file! %+v", roFileOut)
	}
	if !strings.Contains(strings.ToLower(roFileOut.Stdout+roFileOut.Stderr), "read-only file system") {
		t.Fatalf("expected kernel EROFS ('Read-only file system') on repo write, got stdout=%q stderr=%q", roFileOut.Stdout, roFileOut.Stderr)
	}

	roGitOut := verify.RunSandboxedVerificationCommand(
		ctx,
		repo,
		"echo TAMPERED > .git/HEAD",
		10*time.Second,
		verify.EnvPolicy{},
	)
	if roGitOut.ExitCode == 0 {
		t.Fatalf("live Linux bwrap permitted writing to .git/HEAD! %+v", roGitOut)
	}

	afterOracle, err := os.ReadFile(oracleFile)
	if err != nil || string(afterOracle) != oracleOriginal {
		t.Fatalf("oracle_test.go mutated despite bwrap ro-bind! got %q err=%v", string(afterOracle), err)
	}

	// 3. Timeout + process-group reaping of background grandchildren.
	reapToken := "dc_bwrap_reap_canary_" + strconv.FormatInt(time.Now().UnixNano(), 10)
	start := time.Now()
	timedOut := verify.RunSandboxedVerificationCommand(
		ctx,
		repo,
		"/bin/sh -c 'sleep 30' "+reapToken+" &\nsleep 30",
		400*time.Millisecond,
		verify.EnvPolicy{},
	)
	elapsed := time.Since(start)
	if !timedOut.TimedOut || timedOut.ExitCode != -1 {
		t.Fatalf("expected TimedOut=true ExitCode=-1 on hung bwrap command, got %+v", timedOut)
	}
	if elapsed > 6*time.Second {
		t.Fatalf("timed-out bwrap command took %v (expected < 6s with process-group kill)", elapsed)
	}
	deadline := time.Now().Add(2 * time.Second)
	for hasLiveProcessWithToken(reapToken) && time.Now().Before(deadline) {
		time.Sleep(25 * time.Millisecond)
	}
	if hasLiveProcessWithToken(reapToken) {
		t.Fatalf("leaked live background child process %q after bwrap timeout!", reapToken)
	}
}
