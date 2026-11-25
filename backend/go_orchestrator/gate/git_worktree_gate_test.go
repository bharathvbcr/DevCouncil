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

// shellPath spells a path for an unquoted word of a command line. The gate
// parses the line as a shell does, where a backslash escapes the character
// after it, so a Windows path pasted in as is (C:\Users\...) reaches it as
// C:Users... and names nothing. Forward slashes are what git and every POSIX
// shell on Windows accept, and the gate reads them as the same path.
func shellPath(p string) string { return filepath.ToSlash(p) }

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

	// A modelled git write into a sibling worktree (both relative and absolute
	// path) is refused by the sibling allowlist, not by the caller's scope or the
	// caller's outside_root. It used to be allowed because the sibling evaluator
	// checks hard rules only, so `checkout -- docs/notes.md` discarded another
	// session's edits and still passed; the snapshot that added this test
	// pinned that as intended (2026-11-17 Fable ruling replaces it).
	for _, cmd := range []string{
		"git -C ../wt-b checkout -- docs/notes.md",
		"git -C " + shellPath(wtB) + " checkout -- docs/notes.md",
	} {
		d, err := g.EvaluateCommand(cmd, task)
		if err != nil {
			t.Fatal(err)
		}
		if !d.Blocked() || d.Rule != policy.RuleCommandSubstitution || !strings.Contains(d.Reason, "another session's worktree") {
			t.Errorf("%q = %s/%s (%s); want refused by the sibling-worktree allowlist, not by the caller's scope",
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
	outFile := shellPath(filepath.Join(outsideDir, "scratch.json"))
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
			"git -C " + shellPath(wtB) + " apply fix.patch",
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

// 2026-11-17 Fable ruling: in another session's registered worktree git may
// read and record, and nothing else runs. A sibling write is judged against the
// hard rules only, so the modelled writes (`checkout -- .`, `clean`, `restore`)
// discard that session's edits and still pass, and `reset`, `stash`, `switch`,
// `merge`, `rebase`, `pull`, `am`, `read-tree`, `update-ref`, `branch -D` and
// `worktree remove` are modelled by no write rule at all: a deny-list would
// always be incomplete, so the rule is a closed allowlist. Before this, an
// allowed `git *` matched every one of them through `git -C <sibling>`.
func TestGateOnlyLetsGitReadAndRecordInARegisteredSiblingWorktree(t *testing.T) {
	mainRoot, wtB, _ := setupGateWorktreeEnv(t)
	g := newGateAtRoot(t, mainRoot, nil)
	scoped := &dc.Task{
		ID:              "TASK-001",
		PlannedFiles:    []dc.PlannedFile{{Path: "src/calc.go", AllowedChange: dc.ChangeModify}},
		AllowedCommands: []string{"git *"},
	}
	unscoped := &dc.Task{ID: "TASK-002", AllowedCommands: []string{"git *"}}
	refused := []string{
		"reset --hard", "reset --hard HEAD~1", "reset --mixed", "stash", "stash drop", "stash pop",
		"stash push -m x", "clean -fdx", "clean -f docs", "checkout -- docs/notes.md", "checkout main",
		"switch main", "restore docs", "restore --staged docs/notes.md", "commit --amend",
		"commit --amend -m x", "rebase main", "merge feat", "pull", "am fix.patch",
		"read-tree HEAD", "update-ref -d refs/heads/feat", "branch -D feat", "branch newbranch",
		"worktree remove ../x", "rm docs/notes.md", "mv a b", "cherry-pick HEAD", "revert HEAD",
		"gc", "push",
	}
	allowed := []string{
		"status", "status --short", "diff", "diff --stat HEAD", "log --oneline -5", "show HEAD",
		"rev-parse HEAD", "ls-files", "blame README.md", "branch", "branch --list", "branch -a",
		"worktree list", "stash list", "stash show", "apply --check fix.patch", "fetch",
		"add docs/notes.md", "commit -m msg", "describe --tags",
	}
	for name, task := range map[string]*dc.Task{"scoped": scoped, "unscoped": unscoped} {
		for _, sub := range refused {
			for _, target := range []string{"../wt-b", shellPath(wtB)} {
				cmd := "git -C " + target + " " + sub
				d, err := g.EvaluateCommand(cmd, task)
				if err != nil {
					t.Fatal(err)
				}
				if !d.Blocked() || d.Rule != policy.RuleCommandSubstitution || !strings.Contains(d.Reason, "another session's worktree") {
					t.Errorf("%s task: %q = %s/%s (%s); want refused by the sibling-worktree allowlist",
						name, cmd, d.Action, d.Rule, d.Reason)
				}
			}
		}
		// The bare dot is judged against the project root and is refused by the
		// hard path.outside_root rule already, so only "blocked" is asserted: the
		// hole was a NAMED path (`checkout -- docs/notes.md`), listed above.
		for _, sub := range []string{"checkout -- .", "restore ."} {
			cmd := "git -C ../wt-b " + sub
			d, err := g.EvaluateCommand(cmd, task)
			if err != nil {
				t.Fatal(err)
			}
			if !d.Blocked() {
				t.Errorf("%s task: %q = %s (%s); want blocked", name, cmd, d.Action, d.Reason)
			}
		}
		for _, sub := range allowed {
			cmd := "git -C ../wt-b " + sub
			d, err := g.EvaluateCommand(cmd, task)
			if err != nil {
				t.Fatal(err)
			}
			if d.Blocked() {
				t.Errorf("%s task: %q = %s/%s (%s); a sibling worktree may be read and recorded into",
					name, cmd, d.Action, d.Rule, d.Reason)
			}
		}
	}
}
