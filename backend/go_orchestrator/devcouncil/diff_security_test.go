package devcouncil_test

import (
	"context"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil"
)

func TestUntrackedDiffPreservesNativeFilenames(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("backslash is a Windows separator")
	}
	root := newRepo(t)
	for _, name := range []string{`src\a.py`, " padded name.txt "} {
		write(t, root, name, "native filename content\n")
	}
	got, err := devcouncil.GetDiff(context.Background(), root, devcouncil.GetDiffArgs{})
	if err != nil {
		t.Fatal(err)
	}
	diff := mustDiff(t, got)
	if !diff.OK || strings.Count(diff.UnifiedDiff, "+native filename content") != 2 || strings.Contains(diff.UnifiedDiff, "+VALUE = 1") {
		t.Fatalf("native filenames were reinterpreted: %+v", diff)
	}
}

func TestUntrackedDiffRefusesOutsideReads(t *testing.T) {
	for _, kind := range []string{"external symlink", "backslash traversal"} {
		t.Run(kind, func(t *testing.T) {
			if kind == "backslash traversal" && runtime.GOOS == "windows" {
				t.Skip("backslash is a Windows separator")
			}
			root := newRepo(t)
			outside := t.TempDir()
			write(t, outside, "outside.txt", "synthetic external sentinel\n")
			switch kind {
			case "external symlink":
				if err := os.Symlink(filepath.Join(outside, "outside.txt"), filepath.Join(root, "link.txt")); err != nil {
					t.Fatal(err)
				}
			case "backslash traversal":
				rel, err := filepath.Rel(root, filepath.Join(outside, "outside.txt"))
				if err != nil {
					t.Fatal(err)
				}
				write(t, root, strings.ReplaceAll(rel, "/", `\`), "ordinary native filename\n")
			}
			got, err := devcouncil.GetDiff(context.Background(), root, devcouncil.GetDiffArgs{})
			if err != nil {
				t.Fatal(err)
			}
			diff := mustDiff(t, got)
			if diff.OK || diff.Error == "" || strings.Contains(diff.UnifiedDiff, "synthetic external sentinel") {
				t.Fatalf("unsafe untracked read was not refused: %+v", diff)
			}
		})
	}
}

// A secret path is a file to leave out, not a reason to refuse the whole diff:
// one untracked .env used to turn every get_diff into ok:false. It is listed,
// marked, and never opened, and every other file still comes back.
func TestUntrackedSecretIsOmittedAndReported(t *testing.T) {
	root := newRepo(t)
	write(t, root, "secrets/synthetic.txt", "synthetic external sentinel\n")
	write(t, root, ".env", "DB_PASSWORD=hunter2\n")
	write(t, root, "src/new.py", "ordinary untracked content\n")
	if err := os.Symlink(".env", filepath.Join(root, "env-link.txt")); err != nil {
		t.Fatal(err)
	}
	diff := mustDiff(t, getDiff(t, root, devcouncil.GetDiffArgs{}))
	if !diff.OK {
		t.Fatalf("a secret path failed the whole diff: %+v", diff)
	}
	assertAbsent(t, diff, "synthetic external sentinel", "hunter2")
	if !strings.Contains(diff.UnifiedDiff, "+ordinary untracked content") {
		t.Fatalf("the ordinary untracked file was lost: %q", diff.UnifiedDiff)
	}
	assertWithheld(t, diff, "secrets/synthetic.txt", ".env", "env-link.txt")
	assertNotWithheld(t, diff, "src/new.py")
}

// A tracked file under SecretPathPatterns, committed and then modified, used
// to come back in unified_diff whole: only the untracked path consulted the
// read policy.
//
// The repository's own config is the repository's to write, so it sets color
// and prefixes a reader must survive: colored output once hid every section
// header from the filter, which then copied the whole diff through.
func TestTrackedSecretHunksAreWithheld(t *testing.T) {
	for _, staged := range []bool{false, true} {
		t.Run(map[bool]string{false: "worktree", true: "staged"}[staged], func(t *testing.T) {
			root := newRepo(t)
			runGit(t, root, "config", "color.ui", "always")
			runGit(t, root, "config", "color.diff", "always")
			runGit(t, root, "config", "diff.mnemonicPrefix", "true")
			seed := map[string]string{
				".npmrc":                  "registry=https://registry.npmjs.org/\n",
				"secrets/prod.env":        "DB_HOST=db\n",
				"credentials/aws.py":      "REGION = 'us-east-1'\n",
				"deploy/server.KEY":       "header\n",
				"src/b.py":                "B = 1\n",
				"sub dir/secrets/a b.env": "X=1\n",
				"secrets/caf\u00e9.env":   "Y=1\n",
			}
			for name, body := range seed {
				write(t, root, name, body)
			}
			runGit(t, root, "add", "-A")
			runGit(t, root, "commit", "-qm", "seed secrets")
			write(t, root, ".npmrc", "//registry.npmjs.org/:_authToken=npm_SECRETTOKEN\n")
			write(t, root, "secrets/prod.env", "DB_PASSWORD=hunter2\n")
			write(t, root, "credentials/aws.py", "KEY = 'AKIAIOSFODNN7EXAMPLQ'\n")
			write(t, root, "deploy/server.KEY", "xoxb-synthetic-slack-token\n")
			write(t, root, "sub dir/secrets/a b.env", "TOKEN=ghp_syntheticspacedname\n")
			write(t, root, "secrets/caf\u00e9.env", "TOKEN=quoted-name-sentinel\n")
			write(t, root, "src/b.py", "B = 2\n")
			if staged {
				runGit(t, root, "add", "-A")
			}
			diff := mustDiff(t, getDiff(t, root, devcouncil.GetDiffArgs{Staged: staged}))
			if !diff.OK {
				t.Fatalf("get_diff failed: %+v", diff)
			}
			assertAbsent(t, diff, "npm_SECRETTOKEN", "hunter2", "AKIAIOSFODNN7EXAMPLQ", "xoxb-",
				"ghp_syntheticspacedname", "quoted-name-sentinel")
			if !strings.Contains(diff.UnifiedDiff, "+B = 2") {
				t.Fatalf("the ordinary tracked change was lost: %q", diff.UnifiedDiff)
			}
			assertWithheld(t, diff, ".npmrc", "secrets/prod.env", "credentials/aws.py", "deploy/server.KEY")
			assertNotWithheld(t, diff, "src/b.py")
		})
	}
}

// A rename out of a secret directory carries the old file's contents in its
// hunks; the old name alone is enough to withhold them.
func TestRenameOutOfASecretPathIsWithheld(t *testing.T) {
	root := newRepo(t)
	write(t, root, "secrets/prod.env", "DB_PASSWORD=hunter2\nA=1\nB=2\nC=3\n")
	runGit(t, root, "add", "-A")
	runGit(t, root, "commit", "-qm", "seed")
	if err := os.MkdirAll(filepath.Join(root, "config"), 0o755); err != nil {
		t.Fatal(err)
	}
	runGit(t, root, "mv", "secrets/prod.env", "config/prod.txt")
	write(t, root, "config/prod.txt", "DB_PASSWORD=hunter2\nA=1\nB=2\nC=4\n")
	runGit(t, root, "add", "-A")
	diff := mustDiff(t, getDiff(t, root, devcouncil.GetDiffArgs{Staged: true}))
	if !diff.OK {
		t.Fatalf("get_diff failed: %+v", diff)
	}
	assertAbsent(t, diff, "hunter2", "C=3", "C=4")
	assertWithheld(t, diff, "config/prod.txt")
}

func getDiff(t *testing.T, root string, args devcouncil.GetDiffArgs) any {
	t.Helper()
	got, err := devcouncil.GetDiff(context.Background(), root, args)
	if err != nil {
		t.Fatal(err)
	}
	return got
}

func assertAbsent(t *testing.T, diff devcouncil.DiffResult, sentinels ...string) {
	t.Helper()
	for _, s := range sentinels {
		if strings.Contains(diff.UnifiedDiff, s) {
			t.Errorf("unified_diff carries %q:\n%s", s, diff.UnifiedDiff)
		}
	}
}

func assertWithheld(t *testing.T, diff devcouncil.DiffResult, paths ...string) {
	t.Helper()
	for _, p := range paths {
		found := false
		for _, f := range diff.Files {
			if f.Path == p {
				found = true
				if f.Withheld != "secret_path" {
					t.Errorf("%s: want withheld=secret_path, got %+v", p, f)
				}
			}
		}
		if !found {
			t.Errorf("%s is not reported in files: %+v", p, diff.Files)
		}
	}
}

func assertNotWithheld(t *testing.T, diff devcouncil.DiffResult, paths ...string) {
	t.Helper()
	for _, p := range paths {
		for _, f := range diff.Files {
			if f.Path == p && f.Withheld != "" {
				t.Errorf("%s is withheld but is not a secret path: %+v", p, f)
			}
		}
	}
}

func TestUntrackedDiffBoundsInputBeforeReading(t *testing.T) {
	root := newRepo(t)
	f, err := os.Create(filepath.Join(root, "oversize.txt"))
	if err != nil {
		t.Fatal(err)
	}
	if err := f.Truncate(8*1024*1024 + 1); err != nil {
		t.Fatal(err)
	}
	if err := f.Close(); err != nil {
		t.Fatal(err)
	}
	got, err := devcouncil.GetDiff(context.Background(), root, devcouncil.GetDiffArgs{})
	if err != nil {
		t.Fatal(err)
	}
	diff := mustDiff(t, got)
	if diff.OK || !strings.Contains(diff.Error, "limit") {
		t.Fatalf("oversize untracked file was not explicitly refused: %+v", diff)
	}
}
