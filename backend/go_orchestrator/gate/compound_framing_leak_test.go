package gate

import (
	"os"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/flags"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/policy"
)

// Reviewer probes for ft-cb00d69d. Each row runs in strict posture under a task
// whose allowlist admits the command word, so the only thing that can stop it
// is the hard rule for what it does (secret_read, force push, a credential write).
func vLeakRun(t *testing.T, rows []string) {
	t.Helper()
	mainRoot, _, _ := setupGateWorktreeEnv(t)
	if err := os.WriteFile(mainRoot+"/.env", []byte("SECRET=1\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	g := newGateAtRoot(t, mainRoot, map[string]string{flags.HarnessPosture: flags.PostureStrict})
	for _, withTask := range []bool{true, false} {
		task := testTask()
		task.AllowedCommands = []string{"cat *", "git push *", "git diff *", "echo *"}
		if !withTask {
			task = nil
		}
		for _, c := range rows {
			d, err := g.EvaluateCommand(c, task)
			if err != nil {
				t.Fatalf("%q: %v", c, err)
			}
			if !d.Blocked() || d.Severity != policy.Hard {
				t.Errorf("LEAK task=%v %q: want hard refusal, got action=%s rule=%q severity=%v", withTask, c, d.Action, d.Rule, d.Severity)
			}
		}
	}
}

// A reserved word inside a for word list is an item: sh iterates a, do, .env.
func TestForWordListReservedWordIsBoundAsAnItem(t *testing.T) {
	vLeakRun(t, []string{
		"for f in a do .env; do cat $f; done",
		"for f in a do .env; do git diff $f; done",
		"for f in src/calc.go do .env; do echo x > $f; done",
		"for f in a do --force; do git push $f; done",
		"for f in a done .env; do cat $f; done",
	})
}

// The binding caps refuse a line they would have to truncate.
func TestBindingCapsRefuseInsteadOfTruncating(t *testing.T) {
	vLeakRun(t, []string{
		"for g in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16; do echo $g; done; for f in a .env; do cat $f; done",
		"for g in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16; do echo $g; done; for f in a .env; do git diff $f; done",
		"for g in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16; do echo $g; done; for f in x --force; do git push $f; done",
		"for f in a b c d e f g h i j k l m n o p; do :; done; for f in .env; do cat $f; done",
		"for f in a b c d e f g h i j k l m n o p; do :; done; for f in .env; do git diff $f; done",
	})
}
