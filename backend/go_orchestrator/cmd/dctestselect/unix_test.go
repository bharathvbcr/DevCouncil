//go:build !windows

package main

import (
	"bytes"
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

func TestATimeoutKillsTheWholeProcessGroup(t *testing.T) {
	dir := t.TempDir()
	pidfile := filepath.Join(dir, "pid")
	start := time.Now()
	err := runProcess(context.Background(), 1*time.Second, dir, nil, []string{"sh", "-c", "sleep 300 & echo $! > " + pidfile + "; wait"}, &bytes.Buffer{}, &bytes.Buffer{})
	if err == nil {
		t.Fatal("no error from a timed-out command")
	}
	if time.Since(start) > 10*time.Second {
		t.Fatalf("a 1s timeout took %s", time.Since(start))
	}
	raw, _ := os.ReadFile(pidfile)
	pid, _ := strconv.Atoi(strings.TrimSpace(string(raw)))
	if pid == 0 {
		t.Skip("no pid")
	}
	time.Sleep(300 * time.Millisecond)
	if syscall.Kill(pid, 0) == nil {
		// we started this sleep ourselves; clean it up by explicit PID.
		_ = exec.Command("kill", strconv.Itoa(pid)).Run()
		t.Fatalf("grandchild %d survived the timeout", pid)
	}
}
