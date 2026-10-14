//go:build unix

package mcp_test

import (
	"errors"
	"os"
	"os/signal"
	"syscall"
	"testing"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/mcp"
)

// An IDE quitting, launchd stopping a job, or a person pressing Ctrl-C sends the
// host a signal, not an EOF. The host must treat it as the end of the session:
// cancel every running call, wait for what they started to be killed, and
// return. Before it did, a verify's `/bin/sh -c …` lived on in its own process
// group with PPID 1 for up to the command timeout.
func TestASignalCancelsRunningCallsAndTheirProcessGroupsBeforeServeReturns(t *testing.T) {
	for _, sig := range []syscall.Signal{syscall.SIGTERM, syscall.SIGINT} {
		t.Run(sig.String(), func(t *testing.T) {
			// Keeps the test binary alive if the host does not catch the
			// signal, so that case fails as a test instead of killing the run.
			net := make(chan os.Signal, 1)
			signal.Notify(net, sig)
			defer signal.Stop(net)

			h := startLiveHost(t, "sleep 300 & echo $! > PIDFILE; wait")
			_, pid := h.startVerify()

			sent := time.Now()
			if err := syscall.Kill(os.Getpid(), sig); err != nil {
				t.Fatal(err)
			}
			var served error
			select {
			case served = <-h.done:
			case <-time.After(15 * time.Second):
				_ = syscall.Kill(pid, syscall.SIGKILL)
				t.Fatalf("Serve did not return after %v", sig)
			}
			// drainGrace (5 s) bounds the wait for cancelled calls to unwind.
			if took := time.Since(sent); took > 6*time.Second {
				t.Errorf("Serve took %v to return after %v; the drain is bounded at 5s", took, sig)
			}
			// Serve has returned: nothing it started may still be running.
			if err := syscall.Kill(pid, 0); !errors.Is(err, syscall.ESRCH) {
				_ = syscall.Kill(pid, syscall.SIGKILL)
				t.Fatalf("pid %d outlived Serve after %v (kill -0: %v)", pid, sig, err)
			}
			var stopped *mcp.SignalError
			if !errors.As(served, &stopped) || stopped.Signal != sig {
				t.Fatalf("Serve returned %v for %v, want a *mcp.SignalError naming it", served, sig)
			}
		})
	}
}
