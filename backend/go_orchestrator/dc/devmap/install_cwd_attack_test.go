package devmap

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// attackFixture is a repository holding an executable file named devmap that
// records its own execution in marker, and a stand-in devmap outside it that
// records its working directory and arguments.
type attackFixture struct {
	dir, repo, fake, cwdLog, argsLog, marker string
}

func newAttackFixture(t *testing.T) attackFixture {
	t.Helper()
	dir, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	f := attackFixture{
		dir:     dir,
		repo:    filepath.Join(dir, "repo"),
		fake:    filepath.Join(dir, "tools", "devmap"),
		cwdLog:  filepath.Join(dir, "cwd"),
		argsLog: filepath.Join(dir, "args"),
		marker:  filepath.Join(dir, "REPO-FILE-RAN"),
	}
	for _, d := range []string{f.repo, filepath.Dir(f.fake)} {
		if err := os.MkdirAll(d, 0o755); err != nil {
			t.Fatal(err)
		}
	}
	planted := "#!/bin/sh\ntouch '" + f.marker + "'\necho '{\"check_ok\":true}'\n"
	if err := os.WriteFile(filepath.Join(f.repo, "devmap"), []byte(planted), 0o755); err != nil {
		t.Fatal(err)
	}
	script := "#!/bin/sh\npwd -P >> '" + f.cwdLog + "'\nprintf '%s\\n' \"$@\" >> '" + f.argsLog + "'\necho '{\"check_ok\":true}'\n"
	if err := os.WriteFile(f.fake, []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	return f
}

func (f attackFixture) args(t *testing.T) []string {
	t.Helper()
	raw, err := os.ReadFile(f.argsLog)
	if err != nil {
		t.Fatalf("the stand-in devmap did not run: %v", err)
	}
	return strings.Split(strings.TrimSpace(string(raw)), "\n")
}

func (f attackFixture) assertRepoFileNeverRan(t *testing.T) {
	t.Helper()
	if _, err := os.Stat(f.marker); err == nil {
		t.Fatal("the executable in the repository ran")
	}
}

func TestAttackInstallFailsClosedWhenTheTempDirectoryIsMissing(t *testing.T) {
	f := newAttackFixture(t)
	t.Setenv("TMPDIR", filepath.Join(f.dir, "absent"))
	if _, err := New(f.fake, f.repo).Integrate(context.Background(), "claude", InstallCheck, nil, ""); err == nil {
		t.Fatal("an install with no usable working directory reported success")
	}
	if _, err := os.Stat(f.cwdLog); err == nil {
		t.Fatal("devmap ran although its working directory did not exist")
	}
	f.assertRepoFileNeverRan(t)
}

func TestAttackInstallWithTmpdirUnsetRunsInTheSystemTempDirectory(t *testing.T) {
	f := newAttackFixture(t)
	t.Setenv("TMPDIR", "")
	if _, err := New(f.fake, f.repo).Integrate(context.Background(), "claude", InstallCheck, nil, ""); err != nil {
		t.Fatal(err)
	}
	raw, err := os.ReadFile(f.cwdLog)
	if err != nil {
		t.Fatal(err)
	}
	got := strings.TrimSpace(string(raw))
	want, err := filepath.EvalSymlinks("/tmp")
	if err != nil {
		t.Fatal(err)
	}
	if got != want {
		t.Fatalf("ran in %s, want %s", got, want)
	}
}

func TestAttackInstallInAnUnwritableTempDirectoryStillRunsOutsideTheRepository(t *testing.T) {
	if os.Geteuid() == 0 {
		t.Skip("a read-only directory does not constrain root")
	}
	f := newAttackFixture(t)
	locked := filepath.Join(f.dir, "locked")
	if err := os.Mkdir(locked, 0o500); err != nil {
		t.Fatal(err)
	}
	t.Setenv("TMPDIR", locked)
	if _, err := New(f.fake, f.repo).Integrate(context.Background(), "claude", InstallCheck, nil, ""); err != nil {
		t.Fatal(err)
	}
	raw, err := os.ReadFile(f.cwdLog)
	if err != nil {
		t.Fatal(err)
	}
	if got := strings.TrimSpace(string(raw)); got != locked {
		t.Fatalf("ran in %s, want %s", got, locked)
	}
}

// A relative Binary used to be resolved against the repository (cmd.Dir was the
// repository root), so ./devmap named the repository's own file. It is now
// resolved against the temp directory, where it is not found: refused.
func TestAttackInstallNeverRunsARepositoryFileNamedByARelativeBinary(t *testing.T) {
	f := newAttackFixture(t)
	t.Chdir(f.repo)
	for _, binary := range []string{"./devmap", "devmap", "../repo/devmap"} {
		_, _ = New(binary, f.repo).Integrate(context.Background(), "claude", InstallCheck, nil, "")
		f.assertRepoFileNeverRan(t)
	}
}

func TestAttackInstallNeverFindsARepositoryFileThroughADotOnPath(t *testing.T) {
	f := newAttackFixture(t)
	t.Chdir(f.repo)
	for _, path := range []string{".", "", ":/usr/bin", "./"} {
		t.Setenv("PATH", path)
		_, _ = New("devmap", f.repo).Integrate(context.Background(), "claude", InstallCheck, nil, "")
		f.assertRepoFileNeverRan(t)
	}
}

func TestAttackInstallPassesTheAbsoluteRepositoryAndReceiptPathsUnchanged(t *testing.T) {
	f := newAttackFixture(t)
	c := New(f.fake, f.repo)
	only := []string{filepath.Join(f.repo, ".cursor", "mcp.json")}
	if _, err := c.RemoveServers(context.Background(), "cursor", InstallCheck, nil, only); err != nil {
		t.Fatal(err)
	}
	args := f.args(t)
	want := []string{"--project-root", f.repo, "--only-path", only[0]}
	joined := strings.Join(args, "\x00")
	if !strings.Contains(joined, strings.Join(want[:2], "\x00")) || !strings.Contains(joined, strings.Join(want[2:], "\x00")) {
		t.Fatalf("arguments %q do not carry the absolute paths %q", args, want)
	}
}
