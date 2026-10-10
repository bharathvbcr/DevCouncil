package policy

import (
	"os"
	"path/filepath"
	"reflect"
	"sort"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
)

// An allowlisted read-only command is admitted by its leading words, so the
// arguments after them were never judged: `git diff --output=.git/config`
// rewrote repository metadata, `git diff --no-index /dev/null .env` printed a
// credential, and `pytest --junitxml=.env` wrote one, all with no lease. These
// tests hold every spelling of those arguments to a hard refusal.

func readOnlyRoot(t *testing.T) string {
	t.Helper()
	root := t.TempDir()
	if err := os.Mkdir(filepath.Join(root, ".git"), 0o755); err != nil {
		t.Fatal(err)
	}
	for _, name := range []string{".env", "a.go"} {
		if err := os.WriteFile(filepath.Join(root, name), []byte("x"), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	if err := os.Symlink(".env", filepath.Join(root, "link-to-env")); err != nil {
		t.Fatal(err)
	}
	return root
}

func readOnlyTask() *dc.Task {
	return &dc.Task{ID: "t1", AllowedCommands: []string{
		"git diff *", "git log", "git log *", "git show", "git show *", "git status *", "git --no-pager *",
	}}
}

func TestReadOnlyGitArgumentsAreRefusedHard(t *testing.T) {
	root := readOnlyRoot(t)
	gate := CommandGate{HardRules: true, Root: root}

	refused := []string{
		// --output in every spelling, including git's unambiguous abbreviations.
		"git diff --output=../x",
		"git diff --output=.git/config",
		"git diff --output ../x",
		"git diff --outpu=../x",
		"git diff --outp=../x",
		"git diff --out ../x",
		"git diff --ou=../x",
		"git diff --o=../x",
		"git log -p --output=../x",
		"git show HEAD --output=../x",
		"git diff HEAD --output=x -- a.go",
		"git diff --stat --output=x",
		// Quoting that sh removes before git sees the word.
		`git diff "--output=../x"`,
		`git diff --out'put'=../x`,
		`git diff \--output=../x`,
		`git diff $'--output'=../x`,
		"git diff ${OUT:=--output=../x}",
		// Reading arbitrary files.
		"git diff --no-index /dev/null .env",
		"git diff --no-index /dev/null /etc/passwd",
		"git diff --no-index a b",
		"git diff --no-ind /dev/null .env",
		"git diff --no-i /dev/null .env",
		"git diff /dev/null /etc/passwd",
		"git diff /etc/passwd x",
		"git diff -- /etc/passwd",
		"git diff ../outside",
		"git diff -- ../../outside",
		"git diff $HOME/.ssh/id_rsa x",
		"git diff ~/.ssh/id_rsa x",
		// Secret paths named directly.
		"git diff -- .env",
		"git diff HEAD -- .env",
		"git diff HEAD -- config/.env.production",
		"git log -p -- .env",
		"git log -p -- server.key",
		"git show HEAD:.env",
		"git show :.env",
		"git show HEAD:./.env",
		"git show HEAD:config/.env",
		"git diff -- link-to-env",
		"git diff ':(top).env'",
		"git diff ':!.env' .env",
		// External programs run by diff.
		"git diff --ext-diff",
		"git diff --ext HEAD",
		"git log -p --ext-diff",
		"git diff --textconv",
		"git diff --text HEAD",
		"git show --textconv HEAD:a.go",
		"git -c diff.external=/tmp/x diff",
		"git -c diff.external=/tmp/x diff --ext-diff",
		"git -ccore.pager=x diff",
		"git --config-env=diff.external=X diff",
		"git --exec-path=/tmp diff",
		"git --git-dir=/etc diff",
		"git -C /etc diff",
		"GIT_EXTERNAL_DIFF=/tmp/x git diff",
		"GIT_DIR=/etc git diff",
		"env GIT_EXTERNAL_DIFF=/tmp/x git diff",
		// Option files read from disk.
		"git diff -O/etc/passwd",
		"git diff -O ../x",
		"git diff -pO../x",
		"git diff -uO../x",
		"git diff --orderfile=../x",
		"git diff --orderfile ../x",
		"git diff --order=../x",
		"git log -p -O../x",
		// An expansion the gate cannot read.
		"git diff $(echo x)",
		"git diff `echo x`",
		"git diff --$X",
	}
	for _, command := range refused {
		for name, task := range map[string]*dc.Task{"no lease": nil, "lease": readOnlyTask()} {
			d := gate.EvaluateCommand(command, task)
			if d.Action != Deny || d.Severity != Hard {
				t.Errorf("%s: %q = %s/%s (%s); want a hard refusal", name, command, d.Action, d.Severity, d.Rule)
			}
		}
	}
}

func TestReadOnlyGitOutputAbbreviationsAreAllRefused(t *testing.T) {
	gate := CommandGate{HardRules: true, Root: readOnlyRoot(t)}
	for _, option := range []string{"--output", "--no-index", "--ext-diff", "--textconv", "--orderfile"} {
		for n := 3; n <= len(option); n++ {
			abbrev := option[:n]
			for _, tail := range []string{"=x", " x", ""} {
				for _, sub := range []string{"diff", "log -p", "show"} {
					command := "git " + sub + " " + abbrev + tail
					if d := gate.EvaluateCommand(command, readOnlyTask()); d.Action != Deny || d.Severity != Hard {
						t.Errorf("%q = %s/%s; want a hard refusal", command, d.Action, d.Severity)
					}
				}
			}
		}
	}
}

func TestReadOnlyGitOrientationStaysAllowed(t *testing.T) {
	root := readOnlyRoot(t)
	gate := CommandGate{HardRules: true, Root: root}

	// Leaseless: the entries NoTaskAllowedCommands admits.
	for _, command := range []string{
		"git status",
		"git diff",
		"git diff --stat",
		"git diff HEAD~1 -- a.go",
		"git diff --cached",
		"git diff main..feature -- src/a.go",
		"git diff origin/main...HEAD --stat",
		"git diff --no-ext-diff --no-textconv",
		"git diff --name-only --diff-filter=M",
		"git diff -U5 -w -- a.go",
	} {
		d := gate.EvaluateCommand(command, nil)
		if d.Action != Allow {
			t.Errorf("leaseless %q = %s/%s (%s); want allow", command, d.Action, d.Rule, d.Reason)
		}
	}
	// Under a task that lists them.
	for _, command := range []string{
		"git log -p", "git log --oneline -n 5", "git log -p -- a.go", "git log -S foo --stat",
		"git show", "git show HEAD", "git show HEAD --stat", "git show HEAD:a.go",
		"git log --output-indicator-new=+ -p", "git --no-pager diff", "git --no-pager log -p",
		"git status --porcelain",
	} {
		d := gate.EvaluateCommand(command, readOnlyTask())
		if d.Action != Allow {
			t.Errorf("leased %q = %s/%s (%s); want allow", command, d.Action, d.Rule, d.Reason)
		}
	}
}

func TestReadOnlyGitArgumentsRungIsInertForOtherCommands(t *testing.T) {
	gate := CommandGate{HardRules: true, Root: readOnlyRoot(t)}
	task := &dc.Task{ID: "t", AllowedCommands: []string{"git commit *", "echo *"}}
	for _, command := range []string{"git commit -m x", "echo --output=x", "echo git diff --output=x"} {
		if d := gate.EvaluateCommand(command, task); d.Action != Allow {
			t.Errorf("%q = %s/%s (%s); want allow", command, d.Action, d.Rule, d.Reason)
		}
	}
}

func TestPytestOutputOptionsAreWrites(t *testing.T) {
	for _, tc := range []struct {
		command string
		want    []string
		opaque  bool
	}{
		{"pytest --junitxml=.env x", []string{".env"}, false},
		{"pytest --junitxml .env x", []string{".env"}, false},
		{"pytest --junit-xml=r.xml", []string{"r.xml"}, false},
		{"pytest --junitx=.env", []string{".env"}, false},
		{"pytest --basetemp=.git x", []string{".git"}, false},
		{"pytest --basetemp .git x", []string{".git"}, false},
		{"pytest --base=.git", []string{".git"}, false},
		{"python -m pytest --junitxml=../outside.xml x", []string{"../outside.xml"}, false},
		{"python3 -m pytest --junitxml=../outside.xml x", []string{"../outside.xml"}, false},
		{"uv run python -m pytest --junitxml=../o.xml", []string{"../o.xml"}, false},
		{"uv run pytest --basetemp=../o", []string{"../o"}, false},
		{"pytest --resultlog=a.log --report-log=b.log --log-file=c.log", []string{"a.log", "b.log", "c.log"}, false},
		{"pytest --cache-dir=.git/c", []string{".git/c"}, false},
		{"pytest -o cache_dir=.git/c", []string{".git/c"}, false},
		{"pytest -ocache_dir=.git/c", []string{".git/c"}, false},
		{"pytest -qo cache_dir=.git/c", []string{".git/c"}, false},
		{"pytest --override-ini=cache_dir=../c", []string{"../c"}, false},
		{"pytest --override-ini log_file=.env", []string{".env"}, false},
		{"pytest -o addopts=--junitxml=.env", []string{".env"}, false},
		{`pytest -o "addopts=-q --basetemp=../b"`, []string{"../b"}, false},
		{"PYTEST_ADDOPTS=--junitxml=.env pytest", []string{".env"}, false},
		{"env PYTEST_ADDOPTS='--basetemp=../b' pytest", []string{"../b"}, false},
		{"pytest --rootdir=/tmp/x", []string{"/tmp/x/.pytest_cache"}, false},
		{"pytest --rootdir /tmp/x", []string{"/tmp/x/.pytest_cache"}, false},
		{"pytest -c /tmp/x/pytest.ini", []string{"/tmp/x/.pytest_cache"}, false},
		{"pytest --junitxml=$HOME/x.xml", nil, true},
		{"pytest @args.txt", nil, true},
		// Reports under the root, and plain runs, write only what they name.
		{"pytest -q tests/", nil, false},
		{"pytest -k cache tests/", nil, false},
		{"pytest --cache-clear -x", nil, false},
		{"pytest -c pytest.ini --rootdir=.", nil, false},
		{"pytest -- --junitxml=x.py", nil, false},
	} {
		got, opaque, err := RedirectTargets(tc.command)
		if err != nil {
			t.Fatalf("%q: %v", tc.command, err)
		}
		sort.Strings(got)
		want := append([]string(nil), tc.want...)
		sort.Strings(want)
		if len(got) == 0 && len(want) == 0 {
			got, want = nil, nil
		}
		if !reflect.DeepEqual(got, want) || opaque != tc.opaque {
			t.Errorf("%q: targets %v opaque %v; want %v opaque %v", tc.command, got, opaque, want, tc.opaque)
		}
	}
}

func TestPytestWriteTargetsAreJudgedByTheFileRules(t *testing.T) {
	root := readOnlyRoot(t)
	fg := FileGate{Root: root, HardRules: true}
	for _, command := range []string{
		"pytest --junitxml=.env x",
		"pytest --basetemp=.git x",
		"python -m pytest --junitxml=../outside.xml x",
		"pytest --junitxml=link-to-env",
		"pytest --rootdir=/tmp/elsewhere",
	} {
		targets, _, err := RedirectTargets(command)
		if err != nil || len(targets) == 0 {
			t.Fatalf("%q: targets %v err %v", command, targets, err)
		}
		refused := false
		for _, target := range targets {
			d := fg.EvaluateFileChange(target, readOnlyTask(), dc.OpWrite, false)
			refused = refused || (d.Blocked() && d.Severity == Hard)
		}
		if !refused {
			t.Errorf("%q: no target %v was hard-refused by the write rules", command, targets)
		}
	}
}
