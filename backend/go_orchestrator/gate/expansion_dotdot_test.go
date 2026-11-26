package gate

import (
	"os"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/flags"
)

// ft-463dbc1d: filepath.Clean treats an expansion as a directory a later ".."
// cancels, but the shell expands an unset variable to nothing, so the write
// lands somewhere else. Every leak row must be refused (hard, so a dev posture
// blocks it too); every control row must stay allowed.
var expansionDotDotLeaks = []string{
	"echo x > src/$zz/../Makefile",
	"echo x >> src/$zz/../Makefile",
	"echo x > src/$a/$b/$c/../../../outside.txt",
	"echo x > src/${zz:-..}/../x.go",
	"echo x > {src,x}/../Makefile",
	"echo x > ~root/../Makefile",
	"echo x > ${zz:-..}/Makefile",
	"echo x > src/$a/../.git/hooks/pre-commit",
	"echo x | tee src/$zz/../Makefile",
	"cp src/calc.go src/$zz/../Makefile",
	"mv src/calc.go src/$zz/../Makefile",
	"rm src/$zz/../README.md",
	"dd if=src/calc.go of=src/$zz/../Makefile",
	"sed -i s/a/b/ src/$zz/../Makefile",
	"ln -s x src/$zz/../Makefile",
	"mytool src/$zz/../.env",
}

var expansionDotDotControls = []string{
	"echo x > src/calc.go",
	"d=src; echo x > $d/calc.go",
	"d=lib; echo x > src/$d/../calc.go",
}

func TestExpansionBeforeDotDotWriteTargetsRefused(t *testing.T) {
	mainRoot, _, _ := setupGateWorktreeEnv(t)
	os.MkdirAll(mainRoot+"/src", 0o755)
	for _, posture := range []string{flags.PostureStrict, flags.PostureDev} {
		g := newGateAtRoot(t, mainRoot, map[string]string{flags.HarnessPosture: posture})
		for _, withTask := range []bool{true, false} {
			var task *dc.Task
			if withTask {
				task = testTask()
				task.AllowedCommands = []string{"*"}
			}
			for _, cmd := range expansionDotDotLeaks {
				d, err := g.EvaluateCommand(cmd, task)
				if err != nil {
					t.Fatal(err)
				}
				if !d.Blocked() {
					t.Errorf("%s task=%v %q: allowed (%s %v %q)", posture, withTask, cmd, d.Action, d.Severity, d.Rule)
				}
			}
		}
		task := testTask()
		task.AllowedCommands = []string{"*"}
		for _, cmd := range expansionDotDotControls {
			d, err := g.EvaluateCommand(cmd, task)
			if err != nil {
				t.Fatal(err)
			}
			if d.Blocked() {
				t.Errorf("%s control %q: blocked (%s %v %q)", posture, cmd, d.Action, d.Severity, d.Rule)
			}
		}
	}
}
