package main

import (
	"bytes"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

func runArgs(argv ...string) (int, string, string) {
	var stdout, stderr bytes.Buffer
	code := run(argv, &stdout, &stderr)
	return code, stdout.String(), stderr.String()
}

func TestArgumentErrorsAreCouldNotRunNotMismatch(t *testing.T) {
	for _, argv := range [][]string{
		{"--bogus"},
		{"--tag"},
		{"--root"},
		{"--tag-at-head"},
	} {
		if code, _, stderr := runArgs(argv...); code != 2 || !strings.Contains(stderr, "dcrelease:") {
			t.Fatalf("%v: exit %d, stderr %q", argv, code, stderr)
		}
	}
}

func TestHelpExitsZero(t *testing.T) {
	if code, stdout, _ := runArgs("--help"); code != 0 || !strings.Contains(stdout, "Usage") {
		t.Fatalf("exit %d, stdout %q", code, stdout)
	}
}

func TestAMissingRootIsAMismatchNamingEveryFile(t *testing.T) {
	code, _, stderr := runArgs("--root", t.TempDir())
	if code != 1 || !strings.Contains(stderr, "FAIL: missing package.json") {
		t.Fatalf("exit %d, stderr %q", code, stderr)
	}
}

func TestThisRepositoryPassesFromTheModuleDirectory(t *testing.T) {
	// `go -C backend/go_orchestrator run ./cmd/dcrelease` runs here, three
	// levels below the checkout; the root is found by walking up.
	code, stdout, stderr := runArgs()
	if code != 0 || !strings.HasPrefix(stdout, "OK: product version ") {
		t.Fatalf("exit %d\nstdout %s\nstderr %s", code, stdout, stderr)
	}
}

func TestARelativeArtifactsDirectoryIsReadFromTheRoot(t *testing.T) {
	root, err := filepath.Abs(filepath.Join("..", "..", "..", ".."))
	if err != nil {
		t.Fatal(err)
	}
	art := filepath.Join(root, "rust", "target", "dcrelease-test-artifacts-"+filepath.Base(t.TempDir()))
	if err := os.MkdirAll(art, 0o755); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { os.RemoveAll(art) })
	rel, err := filepath.Rel(root, art)
	if err != nil {
		t.Fatal(err)
	}
	// A path resolved against the module directory would not exist and every
	// archive would be named missing; the planted one tells the two apart.
	if err := os.WriteFile(filepath.Join(art, "devmap-cli-aarch64-apple-darwin.tar.xz"), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	code, _, stderr := runArgs("--root", root, "--artifacts", rel)
	if code != 1 || strings.Contains(stderr, "devmap-cli-aarch64-apple-darwin.tar.xz") || !strings.Contains(stderr, "devmap-cli-x86_64-pc-windows-msvc.zip") {
		t.Fatalf("exit %d, stderr %q", code, stderr)
	}
}

func git(t *testing.T, dir string, args ...string) string {
	t.Helper()
	cmd := exec.Command("git", append([]string{"-C", dir, "-c", "user.name=t", "-c", "user.email=t@example.invalid", "-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"}, args...)...)
	out, err := cmd.CombinedOutput()
	if err != nil {
		t.Fatalf("git %v: %v\n%s", args, err, out)
	}
	return strings.TrimSpace(string(out))
}

func TestTagAtHeadRefusesATagBehindHeadBeforeInspecting(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("git is not installed")
	}
	dir := t.TempDir()
	git(t, dir, "init", "-q")
	if err := os.WriteFile(filepath.Join(dir, "a"), []byte("1"), 0o644); err != nil {
		t.Fatal(err)
	}
	git(t, dir, "add", "a")
	git(t, dir, "commit", "-q", "-m", "one")
	git(t, dir, "tag", "v0.2.2")
	tagged := git(t, dir, "rev-parse", "HEAD")
	if err := os.WriteFile(filepath.Join(dir, "a"), []byte("2"), 0o644); err != nil {
		t.Fatal(err)
	}
	git(t, dir, "commit", "-q", "-am", "two")

	code, _, stderr := runArgs("--root", dir, "--tag", "v0.2.2", "--tag-at-head")
	if code != 1 || !strings.Contains(stderr, tagged[:12]) || !strings.Contains(stderr, "docs/releases/v0.2.2.md") {
		t.Fatalf("exit %d, stderr %q", code, stderr)
	}

	// A tag that does not exist yet is fine: the push creates it at HEAD.
	// The tree is not a release checkout, so the identity check then fails —
	// with exit 1 for its own reasons, after the tag check printed OK.
	code, stdout, stderr := runArgs("--root", dir, "--tag", "v0.2.3", "--tag-at-head")
	if code != 1 || !strings.Contains(stdout, "OK: v0.2.3 would publish the notes at HEAD") || !strings.Contains(stderr, "missing package.json") {
		t.Fatalf("exit %d\nstdout %s\nstderr %s", code, stdout, stderr)
	}
}

func TestTagAtHeadOutsideAGitCheckoutCouldNotRun(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("git is not installed")
	}
	dir := t.TempDir()
	t.Setenv("GIT_CEILING_DIRECTORIES", filepath.Dir(dir))
	if code, _, stderr := runArgs("--root", dir, "--tag", "v0.2.2", "--tag-at-head"); code != 2 {
		t.Fatalf("exit %d, stderr %q", code, stderr)
	}
}
