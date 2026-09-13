package stopgate_test

import (
	"context"
	"encoding/json"
	"os/exec"
	"path/filepath"
	"strings"
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
			commands, err := json.Marshal([]string{test.command})
			if err != nil {
				t.Fatal(err)
			}
			sql := "INSERT INTO tasks (id,title,description,planned_files_json,expected_tests_json,status) VALUES ('SECURITY-CHECK','check','','[]','" + strings.ReplaceAll(string(commands), "'", "''") + "','ready');"
			if out, err := exec.Command(testsupport.Tool(t, "sqlite3"), client.DB, sql).CombinedOutput(); err != nil {
				t.Fatalf("task fixture: %v %s", err, out)
			}
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
