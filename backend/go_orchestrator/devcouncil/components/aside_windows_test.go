//go:build windows

package components

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"testing"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
)

const helperSleepEnv = "DEVCOUNCIL_COMPONENTS_HELPER_SLEEP"

// TestHelperProcessSleeps is not a test: run as a copy of this binary, it is
// the "devmap.exe someone is running" that an install has to replace.
func TestHelperProcessSleeps(t *testing.T) {
	if os.Getenv(helperSleepEnv) != "1" {
		t.Skip("helper process only")
	}
	time.Sleep(2 * time.Minute)
}

// TestInstallAtomicReplacesARunningProgram is the upgrade the in-use gap
// broke: Windows refuses a rename over a running program, so installing a
// new devmap.exe while an MCP server ran the old one failed. The running copy
// now moves aside, the new one takes its name, and once the old process exits
// the next install sweeps the aside copy away.
func TestInstallAtomicReplacesARunningProgram(t *testing.T) {
	self, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	image, err := os.ReadFile(self)
	if err != nil {
		t.Fatal(err)
	}
	dir := t.TempDir()
	dest := filepath.Join(dir, "tool.exe")
	if err := os.WriteFile(dest, image, 0o755); err != nil {
		t.Fatal(err)
	}
	src := filepath.Join(t.TempDir(), "new.exe")
	if err := os.WriteFile(src, []byte("new"), 0o755); err != nil {
		t.Fatal(err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Minute)
	defer cancel()
	cmd := exec.CommandContext(ctx, dest, "-test.run=^TestHelperProcessSleeps$")
	proc.ConfigureGroup(cmd)
	cmd.Env = append(os.Environ(), helperSleepEnv+"=1")
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	exited := false
	defer func() {
		if !exited {
			_ = cmd.Process.Kill()
			_ = cmd.Wait()
		}
	}()

	if err := installAtomic(src, dest); err != nil {
		t.Fatalf("install over a running program: %v", err)
	}
	if got, _ := os.ReadFile(dest); string(got) != "new" {
		t.Fatalf("dest holds %d bytes, want the new copy", len(got))
	}
	if left := asideEntries(t, dir); len(left) != 1 {
		t.Fatalf("aside copies while the old program runs: %q, want exactly one", left)
	}

	_ = cmd.Process.Kill()
	_ = cmd.Wait()
	exited = true
	if err := installAtomic(src, dest); err != nil {
		t.Fatal(err)
	}
	assertOnlyEntry(t, dir, "tool.exe")
}
