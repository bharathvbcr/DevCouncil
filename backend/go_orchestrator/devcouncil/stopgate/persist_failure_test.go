package stopgate_test

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/stopgate"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// storeThatCannotRecordRuns is the real dcstore behind a wrapper that refuses
// one command, run-record, and passes every other command through. The task
// reads, gap writes and schema setup all reach the real store; only the
// verification run cannot be recorded.
//
// The client normally keeps one `dcstore serve` child and sends commands over
// its stdin, where a wrapper cannot see them. So the wrapper also answers
// `serve` the way a binary predating it does, with a JSON refusal, which moves
// the client to one process per command (`dcstore --db <path> <command> ...`).
func storeThatCannotRecordRuns(t *testing.T) *store.Client {
	t.Helper()
	real := testsupport.DCStore(t)
	dir := t.TempDir()
	wrapper := filepath.Join(dir, "dcstore")
	script := "#!/bin/sh\n" +
		"if [ \"$3\" = serve ]; then echo '{\"ok\":false,\"error\":\"unknown command: serve\"}'; exit 2; fi\n" +
		"if [ \"$3\" = run-record ]; then echo 'run-record refused by the test wrapper' >&2; exit 1; fi\n" +
		"exec '" + real + "' \"$@\"\n"
	if err := os.WriteFile(wrapper, []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	client := store.New(wrapper, filepath.Join(dir, "state.sqlite"))
	if _, err := client.ActiveLeases(context.Background()); err != nil {
		t.Fatalf("the wrapper must pass ordinary commands through: %v", err)
	}
	return client
}

// Before dc-verify-local-runner-unbounded, VerifyTask discarded Persist's
// error (`_ = Persist(...)`), so a verification the store never recorded was
// returned as an ordinary verdict and the stop gate allowed on it.
func TestAVerificationTheStoreDidNotRecordDoesNotAllow(t *testing.T) {
	root := repoWithWorkingTreeChange(t, "src/app.go", "package app\n")
	client := storeThatCannotRecordRuns(t)
	plantTask(t, client, "TASK-UNRECORDED", "src/app.go")

	out := stopgate.Run(context.Background(), stopgate.Input{
		Root: root, Store: client, TaskID: "TASK-UNRECORDED", GateMode: "advisory",
	})

	if out.Decision.Allow {
		t.Fatalf("decision = %+v; an unrecorded verification must not allow", out.Decision)
	}
	if !strings.Contains(out.Decision.Reason, "not recorded") {
		t.Errorf("reason %q should say the verification was not recorded", out.Decision.Reason)
	}
}
