//go:build unix

package main

import (
	"os"
	"path/filepath"
	"syscall"
	"testing"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/gate"
)

// A named pipe at config.yaml is refused without opening it. It used to be
// opened by the retired-key pre-read, which blocks until a writer appears, with
// the registry's gate lock held.
func TestMCPPolicyCheckRefusesAConfigFIFOWithoutBlocking(t *testing.T) {
	root := gateRepo(t, nil)
	cfg := filepath.Join(root, ".devcouncil", "config.yaml")
	if err := os.Remove(cfg); err != nil {
		t.Fatal(err)
	}
	if err := syscall.Mkfifo(cfg, 0o644); err != nil {
		t.Skipf("no FIFOs here: %v", err)
	}
	reg := devcouncil.NewRegistry(root, nil, nil)
	reg.OpenGate = func() (*gate.Gate, error) { return openGate(root) }
	done := make(chan any, 1)
	go func() { done <- policyCheck(t, reg, ".env") }()
	select {
	case out := <-done:
		if ok, answered := allowed(t, out); answered || ok {
			t.Fatalf("a FIFO config produced a verdict: %+v", out)
		}
	case <-time.After(10 * time.Second):
		t.Fatal("policy_check_write blocked on a FIFO at config.yaml")
	}
}
