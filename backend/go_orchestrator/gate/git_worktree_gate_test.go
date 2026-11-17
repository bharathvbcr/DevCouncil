package gate

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/flags"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/policy"
)

func setupGateWorktreeEnv(t *testing.T) (mainRoot, wtB, outsideDir string) {
	t.Helper()
	base := t.TempDir()
	mainRoot = filepath.Join(base, "main")
	wtB = filepath.Join(base, "wt-b")
	outsideDir = filepath.Join(base, "outside")

	for _, d := range []string{mainRoot, outsideDir} {
		if err := os.MkdirAll(d, 0o755); err != nil {
			t.Fatal(err)
		}
	}

	runGit := func(dir string, args ...string) {
		t.Helper()
		cmd := exec.Command("git", append([]string{"-C", dir}, args...)...)
		cmd.Env = append(os.Environ(),
			"GIT_AUTHOR_NAME=Test",
			"GIT_AUTHOR_EMAIL=test@example.com",
			"GIT_COMMITTER_NAME=Test",
			"GIT_COMMITTER_EMAIL=test@example.com",
		)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git -C %s %v failed: %v\n%s", dir, args, err, out)
		}
	}

	runGit(mainRoot, "init", "-b", "main")
	if err := os.WriteFile(filepath.Join(mainRoot, "README.md"), []byte("main\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	runGit(mainRoot, "add", "README.md")
	runGit(mainRoot, "commit", "-m", "init")
	runGit(mainRoot, "worktree", "add", "-b", "feat", wtB)

	return mainRoot, wtB, outsideDir
}

func newGateAtRoot(t *testing.T, root string, overrides map[string]string) *Gate {
	t.Helper()
	reg := flags.New()
	if err := flags.DefineHarnessFlags(reg); err != nil {
		t.Fatalf("define: %v", err)
	}
	merged := map[string]string{flags.HarnessPosture: flags.PostureStrict}
	for k, v := range overrides {
		merged[k] = v
	}
	if err := reg.LoadConfig(merged); err != nil {
		t.Fatalf("config: %v", err)
	}
	g, err := New(reg, root, nil)
	if err != nil {
		t.Fatalf("new gate: %v", err)
	}
	return g
}

func TestGateRegisteredSiblingWorktreeWritesAndShellRedirects(t *testing.T) {
	mainRoot, wtB, outsideDir := setupGateWorktreeEnv(t)
	g := newGateAtRoot(t, mainRoot, nil)

	task := &dc.Task{
		ID: "TASK-001",
		PlannedFiles: []dc.PlannedFile{
			{Path: "src/calc.go", AllowedChange: dc.ChangeModify},
		},
		AllowedCommands: []string{"git *", "sed *", "jq *", "cat *", "head *", "echo *"},
	}

	// Hard rules on sibling worktree still block secret and restricted writes there.
	for _, tc := range []struct {
		cmd  string
		rule policy.RuleID
	}{
		{"git -C ../wt-b checkout -- .env", policy.RuleSecretPath},
		{"git -C ../wt-b rm .env", policy.RuleSecretPath},
		{"git -C ../wt-b checkout -- .git/hooks/pre-commit", policy.RuleRestrictedPath},
		// Caller shell redirect targeting the sibling directory is still a caller-root write -> outside_root.
		{"git -C ../wt-b status > ../wt-b/out.txt", policy.RuleOutsideRoot},
	} {
		d, err := g.EvaluateCommand(tc.cmd, task)
		if err != nil {
			t.Fatal(err)
		}
		if !d.Blocked() || d.Severity != policy.Hard || d.Rule != tc.rule {
			t.Errorf("%q = %s/%s/%s (%s); want hard %s", tc.cmd, d.Action, d.Rule, d.Severity, d.Reason, tc.rule)
		}
	}

	// Non-hard git -C <sibling> writes (both relative and absolute) must not be blocked by caller's scope or caller's outside_root.
	for _, cmd := range []string{
		"git -C ../wt-b checkout -- docs/notes.md",
		"git -C " + wtB + " checkout -- docs/notes.md",
	} {
		d, err := g.EvaluateCommand(cmd, task)
		if err != nil {
			t.Fatal(err)
		}
		if d.Blocked() {
			t.Errorf("%q = %s (%s): %s; want non-blocked against caller scope",
				cmd, d.Action, d.Rule, d.Reason)
		}
	}

	// Caller redirect inside caller's planned scope alongside git -C <sibling> is allowed.
	d, err := g.EvaluateCommand("git -C ../wt-b status > src/calc.go", task)
	if err != nil {
		t.Fatal(err)
	}
	if !d.Clean() {
		t.Errorf("git -C ../wt-b status > src/calc.go = %s (%s): %s; want clean allow", d.Action, d.Rule, d.Reason)
	}

	// Outside reads via jq/cat/head/sed -n pass the full gate, while sed -i write to outside fails as outside_root.
	outFile := filepath.Join(outsideDir, "scratch.json")
	if err := os.WriteFile(outFile, []byte(`{"a":1}`), 0o644); err != nil {
		t.Fatal(err)
	}
	for _, cmd := range []string{
		"jq . " + outFile,
		"cat " + outFile,
		"head -n 1 " + outFile,
		"sed -n '1p' " + outFile,
	} {
		d, err := g.EvaluateCommand(cmd, task)
		if err != nil {
			t.Fatal(err)
		}
		if !d.Clean() {
			t.Errorf("%q = %s (%s): %s; want clean allow", cmd, d.Action, d.Rule, d.Reason)
		}
	}

	d, err = g.EvaluateCommand("sed -i '' 's/a/b/' "+outFile, task)
	if err != nil {
		t.Fatal(err)
	}
	if !d.Blocked() || d.Rule != policy.RuleOutsideRoot {
		t.Errorf("sed -i outside = %s/%s (%s); want %s", d.Action, d.Rule, d.Reason, policy.RuleOutsideRoot)
	}
}

func TestGateOpaqueAndUnjudgedRefusalTextsNameJudgedAlternative(t *testing.T) {
	mainRoot, _, _ := setupGateWorktreeEnv(t)
	g := newGateAtRoot(t, mainRoot, nil)
	task := &dc.Task{
		ID:              "TASK-001",
		PlannedFiles:    []dc.PlannedFile{{Path: "src/calc.go", AllowedChange: dc.ChangeModify}},
		AllowedCommands: []string{"echo *", "git *"},
	}

	cases := []struct {
		cmd          string
		wantContains []string
	}{
		{
			cmd:          `echo hi > "$(echo out)"`,
			wantContains: []string{"literal", "edit"},
		},
		{
			cmd:          "git apply changes.patch",
			wantContains: []string{"explicit", "edit"},
		},
	}
	for _, tc := range cases {
		d, err := g.EvaluateCommand(tc.cmd, task)
		if err != nil {
			t.Fatal(err)
		}
		if !d.Blocked() {
			t.Fatalf("%q = %s (%s); want blocked", tc.cmd, d.Action, d.Reason)
		}
		if strings.Contains(d.Reason, "..") {
			t.Errorf("%q reason %q must not contain '..'", tc.cmd, d.Reason)
		}
		lower := strings.ToLower(d.Reason)
		for _, sub := range tc.wantContains {
			if !strings.Contains(lower, strings.ToLower(sub)) {
				t.Errorf("%q reason %q missing %q", tc.cmd, d.Reason, sub)
			}
		}
	}
}

// 2026-11-17 review of 7767f7cf: policy records a sibling's Unjudged reason
// (git apply names its targets inside the patch), but the gate only read the
// caller's own, and only for a scope-bound task. So `git -C <sibling> apply`
// ran unexamined in another session's worktree. A sibling's files are never
// the caller's scope, so the refusal must not depend on it.
func TestGateRefusesAnUnjudgedWriteInARegisteredSiblingWorktree(t *testing.T) {
	mainRoot, wtB, _ := setupGateWorktreeEnv(t)
	g := newGateAtRoot(t, mainRoot, nil)
	scoped := &dc.Task{
		ID:              "TASK-001",
		PlannedFiles:    []dc.PlannedFile{{Path: "src/calc.go", AllowedChange: dc.ChangeModify}},
		AllowedCommands: []string{"git *"},
	}
	unscoped := &dc.Task{ID: "TASK-002", AllowedCommands: []string{"git *"}}
	for name, task := range map[string]*dc.Task{"scoped": scoped, "unscoped": unscoped} {
		for _, cmd := range []string{
			"git -C ../wt-b apply fix.patch",
			"git -C " + wtB + " apply fix.patch",
			"git -C ../wt-b apply --apply fix.patch",
		} {
			d, err := g.EvaluateCommand(cmd, task)
			if err != nil {
				t.Fatal(err)
			}
			if !d.Blocked() {
				t.Errorf("%s task: %q = %s (%s); want blocked: it changes files named inside the patch, in another worktree",
					name, cmd, d.Action, d.Reason)
				continue
			}
			lower := strings.ToLower(d.Reason)
			for _, sub := range []string{"explicit", "edit"} {
				if !strings.Contains(lower, sub) {
					t.Errorf("%s task: %q reason %q missing %q", name, cmd, d.Reason, sub)
				}
			}
		}
		// A read-only apply names nothing it changes, so it stays allowed.
		d, err := g.EvaluateCommand("git -C ../wt-b apply --check fix.patch", task)
		if err != nil {
			t.Fatal(err)
		}
		if d.Blocked() {
			t.Errorf("%s task: read-only apply --check = %s (%s): %s; want non-blocked", name, d.Action, d.Rule, d.Reason)
		}
	}
}
