package policy

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
)

func setupRegisteredWorktreeEnv(t *testing.T) (baseDir, mainRoot, wtB, otherRepo, plainDir string) {
	t.Helper()
	baseDir = t.TempDir()
	mainRoot = filepath.Join(baseDir, "main")
	wtB = filepath.Join(baseDir, "wt-b")
	otherRepo = filepath.Join(baseDir, "other-repo")
	plainDir = filepath.Join(baseDir, "plain")

	for _, d := range []string{mainRoot, otherRepo, plainDir} {
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
		out, err := cmd.CombinedOutput()
		if err != nil {
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

	if err := os.MkdirAll(filepath.Join(wtB, "sub"), 0o755); err != nil {
		t.Fatal(err)
	}

	runGit(otherRepo, "init", "-b", "main")
	if err := os.WriteFile(filepath.Join(otherRepo, "a.txt"), []byte("other\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	runGit(otherRepo, "add", "a.txt")
	runGit(otherRepo, "commit", "-m", "init")

	return baseDir, mainRoot, wtB, otherRepo, plainDir
}

func TestGitSiblingWorktreeAllowedAndJudgedAgainstSibling(t *testing.T) {
	baseDir, mainRoot, wtB, _, _ := setupRegisteredWorktreeEnv(t)

	task := &dc.Task{
		ID:              "TASK-WT",
		Title:           "worktree command evaluation",
		AllowedCommands: []string{"git *"},
	}
	gateMain := CommandGate{Root: mainRoot, HardRules: true}

	allowed := []string{
		"git -C ../wt-b status",
		"git -C../wt-b diff --stat",
		// Slashes, as in every other line here: a command line is read the way
		// a shell reads it, where a backslash is an escape, not a separator.
		fmt.Sprintf("git -C %s log -n 5", filepath.ToSlash(wtB)),
		"git -c color.ui=false -C ../wt-b status --short",
		"git -C . status",
		"git -C ../wt-b diff -- README.md",
		"git -C ../wt-b -c core.pager=less diff",
	}
	for _, cmd := range allowed {
		d := gateMain.EvaluateCommand(cmd, task)
		if d.Action != Allow {
			t.Errorf("%q = %s (%s): %s; want allow", cmd, d.Action, d.Rule, d.Reason)
		}
	}

	// No-task allowlisted read-only git commands also judge registered sibling worktrees.
	for _, cmd := range []string{"git -C ../wt-b status", "git -C ../wt-b diff -- README.md"} {
		d := gateMain.EvaluateCommand(cmd, nil)
		if d.Action != Allow {
			t.Errorf("task=nil %q = %s (%s): %s; want allow", cmd, d.Action, d.Rule, d.Reason)
		}
	}

	// Bidirectional: calling from wt-b against main must also be judged.
	gateWtB := CommandGate{Root: wtB, HardRules: true}
	if d := gateWtB.EvaluateCommand("git -C ../main status", task); d.Action != Allow {
		t.Errorf("from wt-b -> main = %s (%s): %s; want allow", d.Action, d.Rule, d.Reason)
	}

	// Read-only git arguments, secret paths, and git-safety rules must be enforced against wt-b.
	outsideWtB := filepath.Join(baseDir, "outside.txt")
	if err := os.WriteFile(outsideWtB, []byte("x\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(wtB, ".env"), []byte("SECRET=1\n"), 0o600); err != nil {
		t.Fatal(err)
	}

	for _, tc := range []struct {
		cmd  string
		rule RuleID
	}{
		{"git -C ../wt-b diff -- .env", RuleCommandGitReadArgument},
		{"git -C ../wt-b show HEAD:.env", RuleCommandGitReadArgument},
		{"git -C ../wt-b diff -- ../outside.txt", RuleCommandGitReadArgument},
		{"git -C ../wt-b diff -- ../main/README.md", RuleCommandGitReadArgument},
		{"git -C ../wt-b diff --output=out.patch", RuleCommandGitReadArgument},
		{"git -C ../wt-b -c diff.external=cat diff", RuleCommandGitReadArgument},
		{"git -C ../wt-b -c core.pager=sh diff", RuleCommandGitReadArgument},
		{"git -C ../wt-b push --force origin feat", RuleCommandForcePush},
		{"git -C ../wt-b commit --no-gpg-sign -m msg", RuleCommandBypassFlag},
	} {
		d := gateMain.EvaluateCommand(tc.cmd, task)
		if d.Action != Deny || d.Severity != Hard || d.Rule != tc.rule {
			t.Errorf("%q = %s/%s/%s (%s); want hard deny %s", tc.cmd, d.Action, d.Rule, d.Severity, d.Reason, tc.rule)
		}
	}

	// Sibling worktree detection failure modes and non-members must still refuse as directory_change.
	forgedNoBackptr := filepath.Join(baseDir, "forged-no-backptr")
	if err := os.MkdirAll(forgedNoBackptr, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(forgedNoBackptr, ".git"),
		[]byte("gitdir: "+filepath.Join(mainRoot, ".git", "worktrees", "wt-b")+"\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	forgedExternal := filepath.Join(baseDir, "forged-external")
	forgedGitDir := filepath.Join(baseDir, "forged-external-gitdir")
	if err := os.MkdirAll(forgedExternal, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(forgedGitDir, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(forgedGitDir, "HEAD"), []byte("ref: refs/heads/main\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(forgedGitDir, "commondir"), []byte(filepath.Join(mainRoot, ".git")+"\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(forgedGitDir, "gitdir"), []byte(filepath.Join(forgedExternal, ".git")+"\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(forgedExternal, ".git"), []byte("gitdir: "+forgedGitDir+"\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	symlinkedWt := filepath.Join(baseDir, "symlinked-wt")
	if err := os.MkdirAll(symlinkedWt, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(filepath.Join(mainRoot, ".git"), filepath.Join(symlinkedWt, ".git")); err != nil {
		t.Fatal(err)
	}

	deniedDirChange := []string{
		"git --git-dir=.git status",
		"git --work-tree=../wt-b status",
		"git -C ../wt-b/sub status",
		"git -C ../other-repo status",
		"git -C ../plain status",
		"git -C ../forged-no-backptr status",
		"git -C ../forged-external status",
		"git -C ../symlinked-wt status",
		"git -C ../wt-b -C ../plain status",
		"git -C /nonexistent/path status",
		"cd ../wt-b && git status",
	}
	for _, cmd := range deniedDirChange {
		d := gateMain.EvaluateCommand(cmd, task)
		if d.Action != Deny || d.Severity != Hard || d.Rule != RuleCommandDirectoryChange {
			t.Errorf("%q = %s/%s/%s (%s); want hard deny %s", cmd, d.Action, d.Rule, d.Severity, d.Reason, RuleCommandDirectoryChange)
		}
	}
}

func TestOutsideReadCommandsAndForLoops(t *testing.T) {
	baseDir, mainRoot, _, _, _ := setupRegisteredWorktreeEnv(t)
	outsideDir := filepath.Join(baseDir, "outside-scratch")
	if err := os.MkdirAll(outsideDir, 0o755); err != nil {
		t.Fatal(err)
	}
	for name, body := range map[string]string{
		"data.json":  `{"ok":true}`,
		"notes.txt":  "hello\nworld\n",
		"a.bin":      "abc",
		"b.bin":      "abd",
		".env":       "API_KEY=secret\n",
		".env.local": "LOCAL=secret\n",
	} {
		if err := os.WriteFile(filepath.Join(outsideDir, name), []byte(body), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	if err := os.Symlink(filepath.Join(outsideDir, ".env"), filepath.Join(outsideDir, "alias.txt")); err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(filepath.Join(mainRoot, ".claude"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(mainRoot, ".claude", "mcp.json"), []byte("{}\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	task := &dc.Task{
		ID:              "TASK-READS",
		AllowedCommands: []string{"jq *", "cat *", "head *", "sed *", "tr *", "cmp *", "awk *", "mytool *", "ls *"},
	}
	g := CommandGate{Root: mainRoot, HardRules: true}

	allowed := []string{
		"jq '.ok' ../outside-scratch/data.json",
		"cat ../outside-scratch/notes.txt",
		"head -n 5 ../outside-scratch/notes.txt",
		"sed -n '1,2p' ../outside-scratch/notes.txt",
		"tr -d '\\r' < ../outside-scratch/notes.txt",
		"tr 'a' 'b' ../outside-scratch/notes.txt",
		"cmp ../outside-scratch/a.bin ../outside-scratch/b.bin",
		`for f in .claude/mcp.json README.md; do jq . "$f"; done`,
		`for f in ../outside-scratch/data.json ../outside-scratch/notes.txt; do cat "$f"; done`,
	}
	for _, cmd := range allowed {
		d := g.EvaluateCommand(cmd, task)
		if d.Action != Allow {
			t.Errorf("%q = %s (%s): %s; want allow", cmd, d.Action, d.Rule, d.Reason)
		}
	}

	// Secret reads outside the repo (and symlinks to outside secrets) must fail closed as command.secret_read.
	secretCmds := []string{
		"jq . ../outside-scratch/.env",
		"cat ../outside-scratch/.env",
		"head -n 1 ../outside-scratch/.env.local",
		"sed -n '1p' ../outside-scratch/.env",
		"cmp ../outside-scratch/a.bin ../outside-scratch/.env",
		"cat ../outside-scratch/alias.txt",
		`for f in ../outside-scratch/.env; do cat "$f"; done`,
	}
	for _, cmd := range secretCmds {
		d := g.EvaluateCommand(cmd, task)
		if d.Action != Deny || d.Severity != Hard || d.Rule != RuleCommandSecretRead {
			t.Errorf("%q = %s/%s/%s (%s); want hard deny %s", cmd, d.Action, d.Rule, d.Severity, d.Reason, RuleCommandSecretRead)
		}
	}

	// Unmodelled programs (including awk, >16-item for loops with protected items, and quoted "for") refuse.
	overLimit := "for f in a1 a2 a3 a4 a5 a6 a7 a8 a9 a10 a11 a12 a13 a14 a15 a16 a17 .claude/mcp.json; do mytool \"$f\"; done"
	unverifiable := []string{
		"awk -f ../outside-scratch/prog.awk ../outside-scratch/notes.txt",
		`for f in ../outside-scratch/data.json; do mytool "$f"; done`,
		`for f in .claude/mcp.json; do mytool "$f"; done`,
		`"for" .claude/mcp.json`,
		overLimit,
	}
	for _, cmd := range unverifiable {
		d := g.EvaluateCommand(cmd, task)
		if d.Action != Deny || d.Severity != Hard || d.Rule != RuleCommandOperandUnverifiable {
			t.Errorf("%q = %s/%s/%s (%s); want hard deny %s", cmd, d.Action, d.Rule, d.Severity, d.Reason, RuleCommandOperandUnverifiable)
		}
	}
}

func TestRefusalTextsNameJudgedAlternativeAndNoDoubleDot(t *testing.T) {
	_, mainRoot, _, _, _ := setupRegisteredWorktreeEnv(t)
	g := CommandGate{Root: mainRoot, HardRules: true}
	task := &dc.Task{
		ID:              "TASK-REMEDY",
		AllowedCommands: []string{"echo *", "git *", "mytool *", "eval *"},
	}

	cases := []struct {
		cmd          string
		wantRule     RuleID
		wantContains []string
	}{
		{
			cmd:          "cd sub && echo x > f.txt",
			wantRule:     RuleCommandDirectoryChange,
			wantContains: []string{"repository root", "git -C"},
		},
		{
			cmd:          "git -C ../plain status",
			wantRule:     RuleCommandDirectoryChange,
			wantContains: []string{"registered", "repository root"},
		},
		{
			cmd:          "echo <<EOF\nhi\nEOF",
			wantRule:     RuleCommandHeredoc,
			wantContains: []string{"file", "edit"},
		},
		{
			cmd:          `eval "git status"`,
			wantRule:     RuleCommandReparse,
			wantContains: []string{"directly"},
		},
		{
			cmd:          "echo $(unterminated",
			wantRule:     RuleCommandSubstitution,
			wantContains: []string{"without $()"},
		},
		{
			cmd:          "mytool /outside/file.txt",
			wantRule:     RuleCommandOperandUnverifiable,
			wantContains: []string{"cat", "jq", "sed -n"},
		},
		{
			cmd:          "mytool .git/config",
			wantRule:     RuleCommandOperandUnverifiable,
			wantContains: []string{"cat", "jq", "sed -n"},
		},
	}

	for _, tc := range cases {
		d := g.EvaluateCommand(tc.cmd, task)
		if d.Action != Deny || d.Rule != tc.wantRule {
			t.Fatalf("%q = %s/%s (%s); want deny %s", tc.cmd, d.Action, d.Rule, d.Reason, tc.wantRule)
		}
		if strings.Contains(d.Reason, "..") {
			t.Errorf("%q reason %q must not contain '..'", tc.cmd, d.Reason)
		}
		lower := strings.ToLower(d.Reason)
		for _, sub := range tc.wantContains {
			if !strings.Contains(lower, strings.ToLower(sub)) {
				t.Errorf("%q reason %q missing expected alternative hint %q", tc.cmd, d.Reason, sub)
			}
		}
	}
}

func TestRedirectWritesInRootSeparatesSiblingGitWritesFromCallerRedirects(t *testing.T) {
	_, mainRoot, wtB, _, _ := setupRegisteredWorktreeEnv(t)
	evalWtB, err := filepath.EvalSymlinks(wtB)
	if err != nil {
		t.Fatal(err)
	}

	set, err := RedirectWritesInRoot("git -C ../wt-b checkout -- docs/notes.md > src/calc.go", mainRoot)
	if err != nil {
		t.Fatal(err)
	}
	if set.Opaque {
		t.Fatalf("expected non-opaque redirect set, got %+v", set)
	}
	if len(set.Targets) != 1 || set.Targets[0] != "src/calc.go" {
		t.Fatalf("caller Targets = %v; want [src/calc.go]", set.Targets)
	}
	if len(set.SiblingWrites) != 1 {
		t.Fatalf("SiblingWrites = %+v; want 1 entry", set.SiblingWrites)
	}
	if !sameWorktreePath(set.SiblingWrites[0].Root, evalWtB) {
		t.Fatalf("SiblingWrites[0].Root = %q; want %q", set.SiblingWrites[0].Root, evalWtB)
	}
	if len(set.SiblingWrites[0].Targets) != 1 || set.SiblingWrites[0].Targets[0] != "docs/notes.md" {
		t.Fatalf("SiblingWrites[0].Targets = %v; want [docs/notes.md]", set.SiblingWrites[0].Targets)
	}

	delSet, err := RedirectWritesInRoot("git -C ../wt-b rm old.go", mainRoot)
	if err != nil {
		t.Fatal(err)
	}
	if len(delSet.Targets) != 0 || len(delSet.SiblingWrites) != 1 {
		t.Fatalf("delete set = %+v; want caller empty and 1 sibling", delSet)
	}
	if len(delSet.SiblingWrites[0].Deletes) != 1 || delSet.SiblingWrites[0].Deletes[0] != "old.go" {
		t.Fatalf("SiblingWrites[0].Deletes = %v; want [old.go]", delSet.SiblingWrites[0].Deletes)
	}
}
