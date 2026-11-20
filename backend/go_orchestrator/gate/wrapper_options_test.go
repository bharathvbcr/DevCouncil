package gate

import (
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/flags"
)

var wrapperTaskCommands = []string{"env *", "sudo *", "nice *", "timeout *", "stdbuf *", "ionice *", "cat *", "git show *", "rm *", "cp *", "sed *", "tee *", "go test *", "go build *", "make *", "ls *", "devcouncil *"}

// ft-46efdb70: a wrapper followed by an option still runs the wrapped command,
// and every operand rung judges that command.
func TestWrapperOptionsReachOperandRungs(t *testing.T) {
	refused := []string{
		"env -i cat .env", "env -u HOME cat .env", "env FOO=1 cat .env", "sudo -u root cat .env",
		"sudo -E cat .env", "nice -n 5 cat .env", "timeout 5 cat .env", "timeout -s KILL 5 cat .env",
		"stdbuf -o0 cat .env", "ionice -c3 cat .env", "env -i git show HEAD:.env",
		"env -i rm -rf .git", "sudo -u root cp x .git/hooks/pre-commit", "env -i sed -i s/a/b/ .env",
		"nice -n 5 tee .git/config", "env -i devcouncil review record", "sudo -u root devcouncil review record",
		"timeout -s KILL 5 devcouncil review record", "env -i dcstore review-append",
		// a wrapper option the table does not know fails closed
		"env --frobnicate cat README.md", "sudo --frobnicate ls",
	}
	allowed := []string{
		"env -i go test ./...", "timeout 60 go build ./...", "nice -n 10 make", "env FOO=1 cat README.md", "sudo -u root ls",
	}
	for _, posture := range []string{flags.PostureStrict, flags.PostureDev} {
		g, _ := landGate(t, posture)
		for _, withTask := range []bool{true, false} {
			var task *dc.Task
			if withTask {
				task = testTask()
				task.AllowedCommands = wrapperTaskCommands
			}
			for _, c := range refused {
				d, err := g.EvaluateCommand(c, task)
				if err != nil {
					t.Fatal(err)
				}
				t.Logf("ROW %s task=%v %q -> %s %q", posture, withTask, c, d.Action, d.Rule)
				if !d.Blocked() {
					t.Errorf("%s task=%v %q: allowed, want a refusal", posture, withTask, c)
				}
			}
			if !withTask {
				continue
			}
			for _, c := range allowed {
				d, err := g.EvaluateCommand(c, task)
				if err != nil {
					t.Fatal(err)
				}
				if d.Blocked() {
					t.Errorf("%s %q: control row refused: %s %q (%s)", posture, c, d.Action, d.Rule, d.Reason)
				}
			}
		}
	}
}
