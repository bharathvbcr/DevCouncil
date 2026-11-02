//go:build unix

package proc

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"
)

func TestConfigureGroupPutsTheChildInItsOwnProcessGroup(t *testing.T) {
	cmd := exec.Command("true")
	ConfigureGroup(cmd)
	if cmd.SysProcAttr == nil || !cmd.SysProcAttr.Setpgid {
		t.Fatal("Setpgid must be set so a timeout can address the group")
	}
	if cmd.Cancel == nil {
		t.Fatal("Cancel must kill the group, not only the direct child")
	}
}

func TestConfigureGroupCancelBeforeStartIsANoop(t *testing.T) {
	cmd := exec.Command("true")
	ConfigureGroup(cmd)
	if err := cmd.Cancel(); err != nil {
		t.Fatalf("Cancel before Start must be a no-op, got %v", err)
	}
}

func TestConfigureGroupKillsGrandchildrenWhenTheParentIsCancelled(t *testing.T) {
	// CommandContext kills the direct child. A child that started its own
	// children leaves them holding the inherited stdout pipe unless the
	// group is signalled. That is the whole reason ConfigureGroup exists, so
	// the assertion is that the grandchild is gone after cancel — not that
	// Wait returned.
	dir := t.TempDir()
	pidFile := filepath.Join(dir, "grandchild.pid")
	script := filepath.Join(dir, "hold.sh")
	body := "#!/bin/sh\n" +
		"sleep 120 &\n" +
		"echo $! > \"$1\"\n" +
		"wait\n"
	if err := os.WriteFile(script, []byte(body), 0o755); err != nil {
		t.Fatal(err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	cmd := exec.CommandContext(ctx, script, pidFile)
	ConfigureGroup(cmd)
	cmd.WaitDelay = 2 * time.Second
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}

	gpid := waitForPIDFile(t, pidFile, 5*time.Second)
	if err := syscall.Kill(gpid, 0); err != nil {
		t.Fatalf("grandchild %d already gone before cancel: %v", gpid, err)
	}

	cancel()
	_ = cmd.Wait()

	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) {
		if err := syscall.Kill(gpid, 0); err != nil {
			return
		}
		time.Sleep(20 * time.Millisecond)
	}
	t.Fatalf("grandchild %d survived cancel; ConfigureGroup did not reach the process group", gpid)
}

func waitForPIDFile(t *testing.T, path string, bound time.Duration) int {
	t.Helper()
	deadline := time.Now().Add(bound)
	for {
		data, err := os.ReadFile(path)
		if err == nil {
			text := strings.TrimSpace(string(data))
			if pid, convErr := strconv.Atoi(text); convErr == nil && pid > 0 {
				return pid
			}
		}
		if time.Now().After(deadline) {
			t.Fatalf("pid file %s was never written", path)
		}
		time.Sleep(20 * time.Millisecond)
	}
}

func TestEnforceSubprocessInvariants(t *testing.T) {
	// The helper sets Cancel, which os/exec accepts only on a CommandContext
	// command (the same contract as ConfigureGroup); both callers build theirs
	// that way.
	cmd := exec.CommandContext(t.Context(), "cat")
	EnforceSubprocessInvariants(cmd)

	if cmd.Stdin == nil {
		t.Fatal("expected Stdin to be redirected to /dev/null")
	}
	if cmd.SysProcAttr == nil || !cmd.SysProcAttr.Setpgid {
		t.Fatal("expected Setpgid to be true")
	}
	if cmd.WaitDelay != 500*time.Millisecond {
		t.Fatalf("expected WaitDelay 500ms, got %v", cmd.WaitDelay)
	}
	if cmd.Cancel == nil {
		t.Fatal("expected Cancel function to be configured")
	}
	hasPromptVar := false
	for _, env := range cmd.Env {
		if env == "GIT_TERMINAL_PROMPT=0" {
			hasPromptVar = true
			break
		}
	}
	if !hasPromptVar {
		t.Fatal("expected GIT_TERMINAL_PROMPT=0 in cmd.Env")
	}
}

// A nil cmd.Env means "inherit the parent's environment". Appending the
// prompt-suppression variables to it must not turn that into "only these two".
func TestEnforceSubprocessInvariantsKeepsTheInheritedEnvironment(t *testing.T) {
	home := os.Getenv("HOME")
	if home == "" {
		t.Skip("the test process has no HOME to inherit")
	}
	cmd := exec.CommandContext(t.Context(), "/bin/sh", "-c", `printf '%s' "$HOME"`)
	EnforceSubprocessInvariants(cmd)
	out, err := cmd.Output()
	if err != nil {
		t.Fatalf("run: %v", err)
	}
	if got := string(out); got != home {
		t.Fatalf("child HOME = %q, want the parent's %q", got, home)
	}
	var prompt string
	for _, kv := range cmd.Env {
		if strings.HasPrefix(kv, "GIT_TERMINAL_PROMPT=") {
			prompt = kv
		}
	}
	if prompt != "GIT_TERMINAL_PROMPT=0" {
		t.Fatalf("last GIT_TERMINAL_PROMPT entry = %q, want GIT_TERMINAL_PROMPT=0", prompt)
	}
}

func TestKillGroupWithEscalation(t *testing.T) {
	cmd := exec.Command("sleep", "30")
	ConfigureOwnGroup(cmd)
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	pid := cmd.Process.Pid
	if err := syscall.Kill(pid, 0); err != nil {
		t.Fatalf("process %d failed to start: %v", pid, err)
	}

	KillGroupWithEscalation(pid, 200*time.Millisecond)
	_ = cmd.Wait()

	if err := syscall.Kill(pid, 0); err == nil {
		t.Fatalf("process %d still alive after escalation", pid)
	}
}
