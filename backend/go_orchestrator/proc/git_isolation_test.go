package proc

import (
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"
	"testing"
)

// plantedRepoWithSubmodule is gitRepo with a checked-out submodule at sub
// whose own .git/config defines a filter driver that records each run, a
// dirty tracked file inside it, and an attributes file sending every file in
// the submodule through that driver.
func plantedRepoWithSubmodule(t *testing.T) (root, marker string) {
	t.Helper()
	root = gitRepo(t)
	src := filepath.Join(t.TempDir(), "sub-src")
	if err := os.MkdirAll(src, 0o755); err != nil {
		t.Fatal(err)
	}
	gitRun(t, src, "init", "-q")
	if err := os.WriteFile(filepath.Join(src, "s.txt"), []byte("s\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	gitRun(t, src, "add", "s.txt")
	gitRun(t, src, "-c", "user.email=a@example.test", "-c", "user.name=a", "commit", "-qm", "s")
	gitRun(t, root, "-c", "protocol.file.allow=always", "submodule", "add", "-q", src, "sub")
	gitRun(t, root, "commit", "-qm", "sub")

	sub := filepath.Join(root, "sub")
	script, marker := markerFilter(t, "submodule-filter")
	gitRun(t, sub, "config", "filter.planted.clean", script)
	gitRun(t, sub, "config", "filter.planted.required", "true")
	subGitDir := strings.TrimSpace(gitOutput(t, sub, "rev-parse", "--absolute-git-dir"))
	if err := os.MkdirAll(filepath.Join(subGitDir, "info"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(subGitDir, "info", "attributes"), []byte("* filter=planted\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	// The same size as the committed content: git cannot tell the file
	// changed from its size, so it hashes it, through the filter.
	if err := os.WriteFile(filepath.Join(sub, "s.txt"), []byte("t\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	return root, marker
}

func gitOutput(t *testing.T, root string, args ...string) string {
	t.Helper()
	cmd := exec.Command("git", args...)
	cmd.Dir = root
	out, err := cmd.Output()
	if err != nil {
		t.Fatalf("git %v: %v", args, err)
	}
	return string(out)
}

func runGitCommand(t *testing.T, root string, args ...string) (string, error) {
	t.Helper()
	cmd, err := GitCommand(t.Context(), root, args...)
	if err != nil {
		return "", err
	}
	out, err := cmd.Output()
	return string(out), err
}

// A submodule's own filter driver does not run when the superproject is
// read: git used to run a status inside the submodule to ask whether its
// working tree was dirty, and that status ran the submodule's filter, which
// the superproject's config listing never sees. The dirty submodule is still
// reported, by a guarded read of its own.
func TestASubmoduleFilterDoesNotRunWhenTheSuperprojectIsRead(t *testing.T) {
	root, marker := plantedRepoWithSubmodule(t)

	// The fixture is only evidence if the unguarded read runs the driver.
	unguarded := exec.Command("git", "diff", "HEAD", "--name-only")
	unguarded.Dir = root
	if out, err := unguarded.CombinedOutput(); err != nil {
		t.Fatalf("unguarded diff: %v\n%s", err, out)
	}
	if _, err := os.Stat(marker); err != nil {
		t.Fatal("the fixture is wrong: an unguarded diff of the superproject did not run the submodule's filter")
	}
	if err := os.Remove(marker); err != nil {
		t.Fatal(err)
	}

	for _, args := range [][]string{
		{"diff", "HEAD"},
		{"diff", "HEAD", "--name-only", "-z"},
		{"diff", "--numstat"},
		{"ls-files", "-z"},
		{"ls-files", "--others", "--exclude-standard", "-z"},
	} {
		out, err := runGitCommand(t, root, args...)
		if err != nil {
			t.Fatalf("git %v: %v", args, err)
		}
		if ran, err := os.ReadFile(marker); err == nil {
			t.Fatalf("git %v ran the submodule's filter: %s", args, ran)
		}
		if args[0] == "diff" && strings.Contains(out, "sub") {
			t.Fatalf("git %v still reported the dirty submodule itself: %q", args, out)
		}
	}

	session, err := NewGitSession(root)
	if err != nil {
		t.Fatal(err)
	}
	dirty, err := session.DirtySubmodules(t.Context())
	if err != nil {
		t.Fatal(err)
	}
	if ran, err := os.ReadFile(marker); err == nil {
		t.Fatalf("the dirtiness read ran the submodule's filter: %s", ran)
	}
	if !slices.Equal(dirty, []string{"sub"}) {
		t.Fatalf("dirty submodules = %q, want [sub]", dirty)
	}
	// Scoped to a pathspec that excludes it, the submodule is not reported.
	if dirty, err := session.DirtySubmodules(t.Context(), "a.txt"); err != nil || len(dirty) != 0 {
		t.Fatalf("dirty submodules within a.txt = %q, %v; want none", dirty, err)
	}
	// Clean again, it is not reported either.
	gitRun(t, filepath.Join(root, "sub"), "-c", "filter.planted.clean=", "-c", "filter.planted.required=false", "checkout", "--", "s.txt")
	if dirty, err := session.DirtySubmodules(t.Context()); err != nil || len(dirty) != 0 {
		t.Fatalf("a clean submodule was reported dirty: %q, %v", dirty, err)
	}
}

// A gitlink whose path the index names as leaving the repository is refused,
// never read.
func TestASubmodulePathOutsideTheRootIsRefused(t *testing.T) {
	session := &GitSession{root: t.TempDir()}
	for _, path := range []string{"../escape", "/abs", "a/../../b", ".."} {
		if _, _, err := session.submoduleDir(path); err == nil {
			t.Errorf("submodule path %q was accepted", path)
		}
	}
	link := filepath.Join(session.root, "linked")
	if err := os.Symlink(t.TempDir(), link); err != nil {
		t.Fatal(err)
	}
	if _, _, err := session.submoduleDir("linked"); err == nil {
		t.Error("a submodule path through a symlink was accepted")
	}
}

// partialClone makes a blobless clone of a two-commit repository whose
// remote then points somewhere only core.sshCommand can reach, and plants
// core.sshCommand and credential.helper as marker scripts. Its index names
// HEAD's files and its working tree has none of them, so `diff HEAD` needs
// blobs the clone never fetched.
func partialClone(t *testing.T) (root, marker string) {
	t.Helper()
	src := gitRepo(t)
	gitRun(t, src, "commit", "-qam", "second")
	gitRun(t, src, "config", "uploadpack.allowFilter", "true")
	root = filepath.Join(t.TempDir(), "clone")
	clone := exec.Command("git", "clone", "-q", "--no-checkout", "--filter=blob:none", "file://"+src, root)
	if out, err := clone.CombinedOutput(); err != nil {
		t.Fatalf("partial clone: %v\n%s", err, out)
	}
	gitRun(t, root, "read-tree", "HEAD")
	dir := t.TempDir()
	marker = filepath.Join(dir, "ran")
	script := filepath.Join(dir, "planted.sh")
	// #nosec G306 -- an executable stand-in; the execute bit is the point.
	if err := os.WriteFile(script, []byte("#!/bin/sh\necho \"$0 $*\" >> \""+marker+"\"\nexit 1\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	gitRun(t, root, "config", "remote.origin.url", "ssh://example.invalid/repo.git")
	gitRun(t, root, "config", "core.sshCommand", script)
	gitRun(t, root, "config", "credential.helper", script)
	return root, marker
}

// A read in a partial clone never fetches a missing object: the fetch runs
// what the repository's config names. The read fails loudly instead of
// answering with an empty diff.
func TestAPartialCloneReadDoesNotFetch(t *testing.T) {
	root, marker := partialClone(t)

	// The fixture is only evidence if an unguarded read fetches.
	unguarded := exec.Command("git", "diff", "HEAD")
	unguarded.Dir = root
	unguarded.Env = append(os.Environ(), "GIT_TERMINAL_PROMPT=0")
	_ = unguarded.Run()
	if _, err := os.Stat(marker); err != nil {
		t.Fatal("the fixture is wrong: an unguarded diff in the partial clone did not try to fetch")
	}
	if err := os.Remove(marker); err != nil {
		t.Fatal(err)
	}

	version, err := DetectGitVersion(t.Context(), mustLookGit(t, root))
	if err != nil {
		t.Fatal(err)
	}
	if !version.HonorsNoLazyFetch() {
		t.Skipf("git %s does not honour GIT_NO_LAZY_FETCH; the refusal test covers it", version)
	}
	for _, args := range [][]string{{"diff", "HEAD"}, {"diff", "HEAD~1", "HEAD"}, {"log", "-p", "-1"}} {
		out, err := runGitCommand(t, root, args...)
		if ran, readErr := os.ReadFile(marker); readErr == nil {
			t.Fatalf("git %v fetched from the promisor remote and ran: %s", args, ran)
		}
		if err == nil {
			t.Fatalf("git %v answered without the objects it needed: %q", args, out)
		}
	}
}

func mustLookGit(t *testing.T, root string) string {
	t.Helper()
	git, err := LookPathOutside("git", root)
	if err != nil {
		t.Fatal(err)
	}
	return git
}

// fakeGit puts a git on PATH that reports version and otherwise runs the
// real git, so the version rule can be driven on any machine.
func fakeGit(t *testing.T, version string) {
	t.Helper()
	real, err := exec.LookPath("git")
	if err != nil {
		t.Skip(err)
	}
	dir := t.TempDir()
	body := "#!/bin/sh\nif [ \"$1\" = version ]; then echo 'git version " + version + "'; exit 0; fi\nexec '" + real + "' \"$@\"\n"
	// #nosec G306 -- an executable stand-in; the execute bit is the point.
	if err := os.WriteFile(filepath.Join(dir, "git"), []byte(body), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", dir+string(os.PathListSeparator)+os.Getenv("PATH"))
}

// D9: a git that does not honour GIT_NO_LAZY_FETCH refuses a partial
// clone's read with a typed error naming its version, and still reads an
// ordinary clone.
func TestAGitThatIgnoresNoLazyFetchRefusesOnlyAPartialClone(t *testing.T) {
	cases := []struct {
		version string
		refuse  bool
		named   string
	}{
		{"2.43.0", true, "2.43.0"},
		{"2.44.0", true, "2.44.0"},
		{"2.43.4", false, ""},
		{"2.44.1", false, ""},
		{"2.45.0 (Apple Git-160)", false, ""},
		// A version this cannot read is treated as one that does not honour
		// the variable: the cautious reading.
		{"unknown", true, "unrecognised version"},
	}
	for _, c := range cases {
		t.Run(c.version, func(t *testing.T) {
			root := gitRepo(t)
			fakeGit(t, c.version)
			for _, args := range [][]string{{"diff", "HEAD"}, {"rev-parse", "HEAD"}} {
				if _, err := runGitCommand(t, root, args...); err != nil {
					t.Fatalf("git %s refused an ordinary clone's %v: %v", c.version, args, err)
				}
			}
			for _, promisor := range [][2]string{
				{"remote.origin.promisor", "true"},
				{"extensions.partialClone", "origin"},
				{"remote.origin.partialCloneFilter", "blob:none"},
			} {
				gitRun(t, root, "config", promisor[0], promisor[1])
				for _, args := range [][]string{{"diff", "HEAD"}, {"rev-parse", "HEAD"}, {"log", "-1"}} {
					_, err := runGitCommand(t, root, args...)
					var refused *LazyFetchUnguardedError
					if got := errors.As(err, &refused); got != c.refuse {
						t.Fatalf("git %s, %s, %v: refused = %v (%v), want %v", c.version, promisor[0], args, got, err, c.refuse)
					}
					if c.refuse && !strings.Contains(err.Error(), c.named) {
						t.Fatalf("the refusal does not name the version: %v", err)
					}
				}
				gitRun(t, root, "config", "--unset", promisor[0])
			}
		})
	}
}

func TestParseGitVersion(t *testing.T) {
	cases := []struct {
		line string
		want string
		ok   bool
	}{
		{"git version 2.54.0 (Apple Git-157)\n", "2.54.0", true},
		{"git version 2.39.5.windows.1", "2.39.5", true},
		{"git version 2.45.GIT", "2.45.0", true},
		{"git version 2.44", "2.44.0", true},
		{"hub version 2.14.2", "", false},
		{"git version x.y", "", false},
	}
	for _, c := range cases {
		v, err := ParseGitVersion(c.line)
		if (err == nil) != c.ok || (c.ok && v.String() != c.want) {
			t.Errorf("ParseGitVersion(%q) = %s, %v; want %s ok=%v", c.line, v, err, c.want, c.ok)
		}
	}
}

func TestHonorsNoLazyFetchMatchesGitsReleases(t *testing.T) {
	honours := map[string]bool{
		"1.9.0": false, "2.38.5": false, "2.39.3": false, "2.39.4": true, "2.40.1": false,
		"2.40.2": true, "2.41.1": true, "2.42.1": false, "2.42.2": true, "2.43.3": false,
		"2.43.4": true, "2.44.0": false, "2.44.1": true, "2.45.0": true, "2.54.0": true, "3.0.0": true,
	}
	for version, want := range honours {
		v, err := ParseGitVersion("git version " + version)
		if err != nil {
			t.Fatal(err)
		}
		if got := v.HonorsNoLazyFetch(); got != want {
			t.Errorf("%s honours GIT_NO_LAZY_FETCH = %v, want %v", version, got, want)
		}
	}
}

// countingGit puts a git on PATH that appends its subcommand to a log and
// runs the real git, and returns the real git for the test's own commands.
func countingGit(t *testing.T) (log, real string) {
	t.Helper()
	real, err := exec.LookPath("git")
	if err != nil {
		t.Skip(err)
	}
	dir := t.TempDir()
	log = filepath.Join(dir, "calls")
	body := "#!/bin/sh\nfor a; do case \"$a\" in -c|-C) skip=1;; -*) ;; *) if [ -z \"$skip\" ]; then echo \"$a\" >> '" +
		log + "'; break; fi; skip=;; esac; done\nexec '" + real + "' \"$@\"\n"
	// #nosec G306 -- an executable stand-in; the execute bit is the point.
	if err := os.WriteFile(filepath.Join(dir, "git"), []byte(body), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", dir+string(os.PathListSeparator)+os.Getenv("PATH"))
	return log, real
}

func configReads(t *testing.T, log string) int {
	t.Helper()
	data, err := os.ReadFile(log)
	if errors.Is(err, os.ErrNotExist) {
		return 0
	}
	if err != nil {
		t.Fatal(err)
	}
	return strings.Count(string(data), "config\n")
}

// One operation reads the repository's config once, and again only when the
// repository's config changes.
func TestASessionReadsTheConfigOncePerChange(t *testing.T) {
	root := gitRepo(t)
	log, real := countingGit(t)
	session, err := NewGitSession(root)
	if err != nil {
		t.Fatal(err)
	}
	run := func(args ...string) {
		t.Helper()
		cmd, err := session.Command(t.Context(), args...)
		if err != nil {
			t.Fatal(err)
		}
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	run("diff", "HEAD")
	run("diff", "--numstat")
	run("ls-files", "--others", "-z")
	if n := configReads(t, log); n != 1 {
		t.Fatalf("three reads in one session read the config %d times, want 1", n)
	}

	script, marker := markerFilter(t, "late")
	if out, err := exec.Command(real, "-C", root, "config", "filter.late.clean", script).CombinedOutput(); err != nil {
		t.Fatalf("git config: %v\n%s", err, out)
	}
	if err := os.WriteFile(filepath.Join(root, ".git", "info", "attributes"), []byte("* filter=late\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("diff", "HEAD")
	if n := configReads(t, log); n != 2 {
		t.Fatalf("a changed config was read %d times in all, want 2", n)
	}
	if ran, err := os.ReadFile(marker); err == nil {
		t.Fatalf("a filter added mid-operation ran: %s", ran)
	}

	// One-off reads are one operation each.
	for range 2 {
		if _, err := runGitCommand(t, root, "diff", "HEAD"); err != nil {
			t.Fatal(err)
		}
	}
	if n := configReads(t, log); n != 4 {
		t.Fatalf("two one-off reads brought config reads to %d, want 4", n)
	}
}

// The version is asked once per git binary per process, however many
// sessions read with it.
func TestTheGitVersionIsAskedOncePerBinary(t *testing.T) {
	root := gitRepo(t)
	log, _ := countingGit(t)
	for range 3 {
		if _, err := runGitCommand(t, root, "rev-parse", "HEAD"); err != nil {
			t.Fatal(err)
		}
	}
	data, err := os.ReadFile(log)
	if err != nil {
		t.Fatal(err)
	}
	if n := strings.Count(string(data), "version\n"); n != 1 {
		t.Fatalf("three reads asked the version %d times, want 1:\n%s", n, data)
	}
	// rev-parse on a git that honours GIT_NO_LAZY_FETCH lists no config.
	if n := strings.Count(string(data), "config\n"); n != 0 {
		t.Fatalf("filter-free reads listed the config %d times, want 0", n)
	}
}
