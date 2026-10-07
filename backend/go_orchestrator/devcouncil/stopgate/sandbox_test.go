package stopgate_test

// TASK-P7-2 / GAP-P7-SANDBOX-ADVERTISED.
//
// `--sandbox docker` used to be accepted, ignored, and written back onto the
// report and the store as the sandbox the run used — while every command ran
// on the host through /bin/sh -c. These drive the real store and a real git
// repository, because the lie lived in what was persisted, and a test of Run's
// return value alone could not see the row.

import (
	"context"
	"os/exec"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/stopgate"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// recordedSandboxes reads back every sandbox the store says a verification of
// this task ran in.
func recordedSandboxes(t *testing.T, db, taskID string) []string {
	t.Helper()
	out, err := exec.Command(testsupport.Tool(t, "sqlite3"), db,
		"SELECT sandbox FROM verification_runs WHERE task_id = '"+taskID+"';").CombinedOutput()
	if err != nil {
		t.Fatalf("reading verification_runs: %v %s", err, out)
	}
	var got []string
	for _, line := range strings.Split(strings.TrimSpace(string(out)), "\n") {
		if line = strings.TrimSpace(line); line != "" {
			got = append(got, line)
		}
	}
	return got
}

func TestAnIsolationThisHostDoesNotHaveIsRefusedAndNothingIsRecorded(t *testing.T) {
	for _, requested := range []string{"docker", "nix", "Docker", "firecracker"} {
		t.Run(requested, func(t *testing.T) {
			root := repoWithWorkingTreeChange(t, "src/app.go", "package app\n")
			client := newStore(t)
			plantTask(t, client, "TASK-SANDBOX", "src/app.go")

			out := stopgate.Run(context.Background(), stopgate.Input{
				Root: root, Store: client, TaskID: "TASK-SANDBOX",
				GateMode: "enforce", Sandbox: requested,
			})

			if !out.Decision.Skipped || out.Decision.Allow {
				t.Fatalf("sandbox %q: decision = %+v, want a refusal that is skipped and does not allow",
					requested, out.Decision)
			}
			if !strings.Contains(out.Decision.SkipReason, requested) {
				t.Errorf("the refusal must name what was asked for: %q", out.Decision.SkipReason)
			}
			if out.MCP.Sandbox != "" {
				t.Errorf("a refused run produced a report labelled %q", out.MCP.Sandbox)
			}
			// The persisted half of the original lie: a run row saying
			// sandbox=docker for commands that executed on the host.
			if rows := recordedSandboxes(t, client.DB, "TASK-SANDBOX"); len(rows) != 0 {
				t.Errorf("a refused sandbox still recorded verification runs: %v", rows)
			}
		})
	}
}

// The positive control: the one sandbox that exists still runs, and what is
// recorded is what ran.
func TestTheLocalSandboxRunsAndRecordsLocal(t *testing.T) {
	for _, requested := range []string{"", "local", " local "} {
		root := repoWithWorkingTreeChange(t, "src/app.go", "package app\n")
		client := newStore(t)
		plantTask(t, client, "TASK-LOCAL", "src/app.go")

		out := stopgate.Run(context.Background(), stopgate.Input{
			Root: root, Store: client, TaskID: "TASK-LOCAL",
			GateMode: "enforce", Sandbox: requested,
		})
		if out.Decision.Skipped {
			t.Fatalf("sandbox %q was refused: %+v", requested, out.Decision)
		}
		if out.MCP.Sandbox != verify.SandboxLocal {
			t.Errorf("sandbox %q: report says %q, want %q", requested, out.MCP.Sandbox, verify.SandboxLocal)
		}
		rows := recordedSandboxes(t, client.DB, "TASK-LOCAL")
		if len(rows) != 1 || rows[0] != verify.SandboxLocal {
			t.Errorf("sandbox %q: recorded %v, want exactly [%s]", requested, rows, verify.SandboxLocal)
		}
	}
}
