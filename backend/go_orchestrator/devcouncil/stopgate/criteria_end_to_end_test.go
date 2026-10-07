package stopgate_test

// The shipped path for acceptance-criterion dispatch: a real store holding a
// task that claims a criterion, the requirement row that defines it, and a
// real git repository. verify's own tests hand Run the requirements directly;
// this proves VerifyTask reads them from the store at all.

import (
	"context"
	"os/exec"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/stopgate"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

func linkCriterion(t *testing.T, db, taskID, method string) {
	t.Helper()
	sql := "UPDATE tasks SET requirement_ids_json = '[\"REQ-1\"]', " +
		"acceptance_criterion_ids_json = '[\"AC-1\"]', expected_tests_json = '[\"true\"]' " +
		"WHERE id = '" + taskID + "';" +
		"INSERT INTO requirements (id,title,description,priority,source,acceptance_criteria_json) " +
		"VALUES ('REQ-1','r','','high','user','[{\"id\":\"AC-1\",\"description\":\"d\"," +
		"\"verification_method\":\"" + method + "\"}]');"
	if out, err := exec.Command(testsupport.Tool(t, "sqlite3"), db, sql).CombinedOutput(); err != nil {
		t.Fatalf("criterion fixture: %v %s", err, out)
	}
}

func TestTheShippedHostDispatchesOnVerificationMethod(t *testing.T) {
	cases := []struct {
		method      string
		wantStatus  string
		wantGapType string
	}{
		{"manual", "blocked", "unsupported_verification_method"},
		{"unit_test", "verified", "coarse_acceptance_proof"},
		// Stored before llm_review was refused: the row can no longer be
		// read, which blocks rather than verifying a criterion nobody checked.
		{"llm_review", "blocked", "acceptance_criteria_unproven"},
	}
	for _, tc := range cases {
		t.Run(tc.method, func(t *testing.T) {
			root := repoWithWorkingTreeChange(t, "src/app.go", "package app\n")
			client := newStore(t)
			plantTask(t, client, "TASK-AC", "src/app.go")
			linkCriterion(t, client.DB, "TASK-AC", tc.method)

			out := stopgate.Run(context.Background(), stopgate.Input{
				Root: root, Store: client, TaskID: "TASK-AC", GateMode: "enforce",
			})
			if out.Decision.Skipped {
				t.Fatalf("verify did not run: %+v", out.Decision)
			}
			if out.MCP.Status != tc.wantStatus {
				t.Errorf("status = %s, want %s; gaps=%+v", out.MCP.Status, tc.wantStatus, out.Gaps)
			}
			found := false
			for _, g := range out.Gaps {
				if g.GapType == tc.wantGapType {
					found = true
				}
			}
			if !found {
				t.Errorf("no %s gap: %+v", tc.wantGapType, out.Gaps)
			}
		})
	}
}
