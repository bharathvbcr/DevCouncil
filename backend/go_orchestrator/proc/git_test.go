package proc

import (
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"slices"
	"strings"
	"testing"
)

func TestGitArgsPlacesGuards(t *testing.T) {
	global := []string{
		"-c", "core.fsmonitor=false",
		"-c", "core.hooksPath=" + os.DevNull,
		"-c", "log.showSignature=false",
	}
	cases := []struct {
		name string
		in   []string
		want []string
	}{
		{"diff gets both", []string{"diff", "HEAD"},
			[]string{"diff", "--no-ext-diff", "--no-textconv", "--no-color", "HEAD"}},
		{"blame gets textconv only", []string{"blame", "--line-porcelain", "HEAD", "--", "a"},
			[]string{"blame", "--no-textconv", "--line-porcelain", "HEAD", "--", "a"}},
		{"log and show", []string{"show", "--name-only", "x"},
			[]string{"show", "--no-ext-diff", "--no-textconv", "--no-color", "--name-only", "x"}},
		{"caller globals stay before the subcommand",
			[]string{"-c", "core.quotePath=false", "-C", "sub", "--no-pager", "diff", "--unified=0"},
			[]string{"-c", "core.quotePath=false", "-C", "sub", "--no-pager", "diff", "--no-ext-diff", "--no-textconv", "--no-color", "--unified=0"}},
		{"a subcommand without guards is unchanged", []string{"ls-files", "-z"}, []string{"ls-files", "-z"}},
		{"an argument after the subcommand named diff is not a subcommand",
			[]string{"rev-parse", "diff"}, []string{"rev-parse", "diff"}},
		{"a dangling -c is kept, not overrun", []string{"-c"}, []string{"-c"}},
		{"flags only", []string{"--version"}, []string{"--version"}},
		{"empty", nil, nil},
	}
	for _, c := range cases {
		got := GitArgs(c.in)
		want := append(slices.Clone(global), c.want...)
		if !slices.Equal(got, want) {
			t.Errorf("%s: GitArgs(%q)\n got %q\nwant %q", c.name, c.in, got, want)
		}
	}
}

// The caller's slice is not modified: guards go into a new one.
func TestGitArgsDoesNotAliasItsInput(t *testing.T) {
	in := make([]string, 2, 16)
	in[0], in[1] = "diff", "HEAD"
	_ = GitArgs(in)
	if !slices.Equal(in, []string{"diff", "HEAD"}) || !slices.Equal(in[:cap(in)][2:4], []string{"", ""}) {
		t.Fatalf("input changed: %q", in[:cap(in)])
	}
}

// gitRepo is a one-commit repository with a.txt modified, isolated from the
// machine's system and global config.
func gitRepo(t *testing.T) string {
	t.Helper()
	if runtime.GOOS == "windows" {
		t.Skip("the planted programs are shell scripts")
	}
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip(err)
	}
	t.Setenv("GIT_CONFIG_NOSYSTEM", "1")
	t.Setenv("GIT_CONFIG_GLOBAL", os.DevNull)
	root := t.TempDir()
	for _, args := range [][]string{
		{"init", "-q"},
		{"config", "user.email", "a@example.test"},
		{"config", "user.name", "a"},
	} {
		gitRun(t, root, args...)
	}
	if err := os.WriteFile(filepath.Join(root, "a.txt"), []byte("one\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	gitRun(t, root, "add", "a.txt")
	gitRun(t, root, "commit", "-qm", "base")
	if err := os.WriteFile(filepath.Join(root, "a.txt"), []byte("one\ntwo\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	return root
}

func gitRun(t *testing.T, root string, args ...string) {
	t.Helper()
	cmd := exec.Command("git", args...)
	cmd.Dir = root
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("git %v: %v\n%s", args, err, out)
	}
}

// markerFilter writes a clean filter that records its name and passes
// content through unchanged, so a diff through it is still correct.
func markerFilter(t *testing.T, name string) (script, marker string) {
	t.Helper()
	dir := t.TempDir()
	marker = filepath.Join(dir, "ran")
	script = filepath.Join(dir, name+".sh")
	body := "#!/bin/sh\necho " + name + " >> \"" + marker + "\"\ncat\n"
	// #nosec G306 -- an executable stand-in; the execute bit is the point.
	if err := os.WriteFile(script, []byte(body), 0o755); err != nil {
		t.Fatal(err)
	}
	return script, marker
}

func diffThroughGitCommand(t *testing.T, root string) string {
	t.Helper()
	cmd, err := GitCommand(t.Context(), root, "diff", "HEAD")
	if err != nil {
		t.Fatal(err)
	}
	out, err := cmd.Output()
	if err != nil {
		t.Fatalf("diff: %v", err)
	}
	return string(out)
}

// A filter the repository's own config defines does not run, even marked
// required, and the diff is still the true one.
func TestGitCommandNeutralizesRepositoryFilters(t *testing.T) {
	root := gitRepo(t)
	script, marker := markerFilter(t, "local")
	gitRun(t, root, "config", "filter.planted.clean", script)
	gitRun(t, root, "config", "filter.planted.process", script)
	gitRun(t, root, "config", "filter.planted.required", "true")
	if err := os.WriteFile(filepath.Join(root, ".git", "info", "attributes"), []byte("* filter=planted\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	diff := diffThroughGitCommand(t, root)
	if ran, err := os.ReadFile(marker); err == nil {
		t.Fatalf("the repository's filter ran: %s", ran)
	}
	if !strings.Contains(diff, "+two") {
		t.Fatalf("diff lost the change: %q", diff)
	}
}

// The same holds when the definition reaches .git/config through an
// include of a file the repository tracks.
func TestGitCommandNeutralizesIncludedRepositoryFilters(t *testing.T) {
	root := gitRepo(t)
	script, marker := markerFilter(t, "included")
	cfg := "[filter \"planted\"]\n\tclean = " + script + "\n"
	if err := os.WriteFile(filepath.Join(root, "tracked.cfg"), []byte(cfg), 0o644); err != nil {
		t.Fatal(err)
	}
	gitRun(t, root, "config", "include.path", "../tracked.cfg")
	if err := os.WriteFile(filepath.Join(root, ".git", "info", "attributes"), []byte("a.txt filter=planted\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	diffThroughGitCommand(t, root)
	if ran, err := os.ReadFile(marker); err == nil {
		t.Fatalf("an included repository filter ran: %s", ran)
	}
}

// A filter the operator installed globally (git-lfs, by default) keeps
// working: only repository-scoped definitions are neutralized.
func TestGitCommandKeepsGlobalFilters(t *testing.T) {
	root := gitRepo(t)
	script, marker := markerFilter(t, "global")
	global := filepath.Join(t.TempDir(), "gitconfig")
	if err := os.WriteFile(global, []byte("[filter \"lfs\"]\n\tclean = "+script+"\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	t.Setenv("GIT_CONFIG_GLOBAL", global)
	if err := os.WriteFile(filepath.Join(root, ".git", "info", "attributes"), []byte("* filter=lfs\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	diffThroughGitCommand(t, root)
	if ran, err := os.ReadFile(marker); err != nil || !strings.Contains(string(ran), "global") {
		t.Fatalf("the operator's global filter did not run (err %v)", err)
	}
}

// A repository that redefines a global filter gets the operator's
// definition back, not its own and not none.
func TestGitCommandRestoresTheGlobalDefinitionOverARepositoryOverride(t *testing.T) {
	root := gitRepo(t)
	globalScript, globalMarker := markerFilter(t, "global")
	localScript, localMarker := markerFilter(t, "local")
	global := filepath.Join(t.TempDir(), "gitconfig")
	if err := os.WriteFile(global, []byte("[filter \"lfs\"]\n\tclean = "+globalScript+"\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	t.Setenv("GIT_CONFIG_GLOBAL", global)
	gitRun(t, root, "config", "filter.lfs.clean", localScript)
	if err := os.WriteFile(filepath.Join(root, ".git", "info", "attributes"), []byte("* filter=lfs\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	diffThroughGitCommand(t, root)
	if ran, err := os.ReadFile(localMarker); err == nil {
		t.Fatalf("the repository's override ran: %s", ran)
	}
	if _, err := os.ReadFile(globalMarker); err != nil {
		t.Fatalf("the operator's definition was not restored: %v", err)
	}
}

// A driver name a -c argument cannot spell is refused rather than left
// running.
func TestGitCommandRefusesAFilterNameItCannotNeutralize(t *testing.T) {
	root := gitRepo(t)
	script, _ := markerFilter(t, "odd")
	cfg := "[filter \"we=ird\"]\n\tclean = " + script + "\n"
	if err := os.WriteFile(filepath.Join(root, ".git", "config.extra"), []byte(cfg), 0o644); err != nil {
		t.Fatal(err)
	}
	gitRun(t, root, "config", "include.path", "config.extra")

	if _, err := GitCommand(t.Context(), root, "diff", "HEAD"); err == nil {
		t.Fatal("a filter that cannot be neutralized was left to run")
	}
}

func TestFilterOverridesParsesListings(t *testing.T) {
	entry := func(scope, kv string) string { return scope + "\x00" + kv + "\x00" }
	cases := []struct {
		name    string
		listing string
		want    []string
		wantErr bool
	}{
		{"empty", "", nil, false},
		{"operator only is left alone",
			entry("global", "filter.lfs.clean\ngit-lfs clean -- %f") + entry("global", "filter.lfs.required\ntrue"),
			nil, false},
		{"repository driver is blanked, required forced false",
			entry("local", "filter.x.clean\n/evil"),
			[]string{"-c", "filter.x.clean=", "-c", "filter.x.required=false"}, false},
		{"worktree scope is the repository's too",
			entry("worktree", "filter.x.process\n/evil"),
			[]string{"-c", "filter.x.process=", "-c", "filter.x.required=false"}, false},
		{"the operator's value is restored, the last one winning",
			entry("system", "filter.lfs.clean\nold") + entry("global", "filter.lfs.clean\ngit-lfs clean -- %f") +
				entry("global", "filter.lfs.required\ntrue") + entry("local", "filter.lfs.clean\n/evil"),
			[]string{"-c", "filter.lfs.clean=git-lfs clean -- %f", "-c", "filter.lfs.required=true"}, false},
		{"a command-scope setting is the operator's",
			entry("local", "filter.x.smudge\n/evil") + entry("command", "filter.x.smudge\ncat"),
			[]string{"-c", "filter.x.smudge=cat", "-c", "filter.x.required=false"}, false},
		{"a valueless operator key stays valueless",
			entry("global", "filter.x.required") + entry("local", "filter.x.clean\n/evil"),
			[]string{"-c", "filter.x.clean=", "-c", "filter.x.required"}, false},
		{"a dotted driver name is one name",
			entry("local", "filter.a.b.clean\n/evil"),
			[]string{"-c", "filter.a.b.clean=", "-c", "filter.a.b.required=false"}, false},
		{"drivers come out sorted",
			entry("local", "filter.z.clean\n1") + entry("local", "filter.a.clean\n2"),
			[]string{"-c", "filter.a.clean=", "-c", "filter.a.required=false", "-c", "filter.z.clean=", "-c", "filter.z.required=false"}, false},
		{"a name -c cannot spell is refused", entry("local", "filter.we=ird.clean\n/evil"), nil, true},
		{"a truncated trailing record is ignored", entry("local", "filter.x.clean\n/evil") + "local",
			[]string{"-c", "filter.x.clean=", "-c", "filter.x.required=false"}, false},
		{"a key that is not a driver setting is ignored", entry("local", "filter.\n/evil") + entry("local", "filters.x.clean\n1"), nil, false},
	}
	for _, c := range cases {
		got, err := filterOverrides([]byte(c.listing))
		if (err != nil) != c.wantErr {
			t.Errorf("%s: err = %v, wantErr %v", c.name, err, c.wantErr)
			continue
		}
		if !slices.Equal(got, c.want) {
			t.Errorf("%s:\n got %q\nwant %q", c.name, got, c.want)
		}
	}
}

// The guards reach the git a diff starts inside a submodule: they travel to
// child gits through GIT_CONFIG_PARAMETERS. A submodule's own fsmonitor ran
// during an unguarded diff of its parent and does not run here.
func TestGitCommandGuardsReachSubmoduleGits(t *testing.T) {
	root := gitRepo(t)
	sub := filepath.Join(t.TempDir(), "sub-src")
	if err := os.MkdirAll(sub, 0o755); err != nil {
		t.Fatal(err)
	}
	gitRun(t, sub, "init", "-q")
	gitRun(t, sub, "-c", "user.email=a@example.test", "-c", "user.name=a", "commit", "-q", "--allow-empty", "-m", "s")
	if err := os.WriteFile(filepath.Join(sub, "s.txt"), []byte("s\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	gitRun(t, sub, "add", "s.txt")
	gitRun(t, sub, "-c", "user.email=a@example.test", "-c", "user.name=a", "commit", "-qm", "s")
	gitRun(t, root, "-c", "protocol.file.allow=always", "submodule", "add", "-q", sub, "sub")
	gitRun(t, root, "commit", "-qm", "sub")
	script, marker := markerFilter(t, "fsmonitor")
	gitRun(t, filepath.Join(root, "sub"), "config", "core.fsmonitor", script)
	if err := os.WriteFile(filepath.Join(root, "sub", "s.txt"), []byte("s\ndirty\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	unguarded := exec.Command("git", "diff", "--stat")
	unguarded.Dir = root
	if out, err := unguarded.CombinedOutput(); err != nil {
		t.Fatalf("unguarded diff: %v\n%s", err, out)
	}
	if _, err := os.Stat(marker); err != nil {
		t.Skip("this git does not run a submodule's fsmonitor from its parent's diff; nothing to guard")
	}
	if err := os.Remove(marker); err != nil {
		t.Fatal(err)
	}

	cmd, err := GitCommand(t.Context(), root, "diff", "--stat")
	if err != nil {
		t.Fatal(err)
	}
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("guarded diff: %v\n%s", err, out)
	}
	if ran, err := os.ReadFile(marker); err == nil {
		t.Fatalf("the submodule's fsmonitor ran under the guards: %s", ran)
	}
}
