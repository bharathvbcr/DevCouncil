package stopgate_test

import (
	"context"
	"encoding/json"
	"os/exec"
	"path/filepath"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/stopgate"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

func TestRealStoreStopGateCannotPassAFailedCommandFromItsOutput(t *testing.T) {
	for _, test := range []struct {
		name, command string
		passed        bool
	}{
		{"forged output", "printf 'no tests ran\\n'; exit 1", false},
		{"successful command", "exit 0", true},
	} {
		t.Run(test.name, func(t *testing.T) {
			root := t.TempDir()
			git := exec.Command(testsupport.Tool(t, "git"), "init", "-q", root)
			if out, err := git.CombinedOutput(); err != nil {
				t.Fatalf("git fixture: %v %s", err, out)
			}
			client := store.New(testsupport.DCStore(t), filepath.Join(t.TempDir(), "state.sqlite"))
			ctx := context.Background()
			if _, err := client.ActiveLeases(ctx); err != nil {
				t.Fatal(err)
			}
			scope, err := json.Marshal(map[string]any{"expected_tests": []string{test.command}})
			if err != nil {
				t.Fatal(err)
			}
			testsupport.SeedTask(t, client.DB, testsupport.TaskSeed{ID: "SECURITY-CHECK", Title: "check", Scope: string(scope)})
			out := stopgate.Run(ctx, stopgate.Input{Root: root, Store: client, TaskID: "SECURITY-CHECK", GateMode: "enforce"})
			if out.Decision.Allow != test.passed || out.MCP.Passed != test.passed || out.Decision.Skipped {
				t.Fatalf("store-backed command result: %+v", out)
			}
			if !test.passed {
				found := false
				for _, gap := range out.Gaps {
					if gap.GapType == "test_failed" && gap.Blocking {
						found = true
					}
				}
				if !found {
					t.Fatalf("failed command did not reach the stop gate as a blocking test failure: %+v", out)
				}
			}
		})
	}
}
