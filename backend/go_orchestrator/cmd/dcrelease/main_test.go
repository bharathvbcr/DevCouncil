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

// The git that answers rev-parse is never one the checkout carries. A
// planted git on PATH inside the root could otherwise name any commit as
// HEAD and pass the tag check, and it would run with the releaser's
// authority. It is refused and never started; the check could not run.
func TestTagAtHeadNeverRunsAGitInsideTheRoot(t *testing.T) {
	root := t.TempDir()
	if err := os.Mkdir(filepath.Join(root, ".git"), 0o755); err != nil {
		t.Fatal(err)
	}
	marker := filepath.Join(t.TempDir(), "planted-git-ran")
	bin := filepath.Join(root, "tools", "bin")
	if err := os.MkdirAll(bin, 0o755); err != nil {
		t.Fatal(err)
	}
	script := "#!/bin/sh\n: > '" + marker + "'\necho 0123456789abcdef0123456789abcdef01234567\n"
	if err := os.WriteFile(filepath.Join(bin, "git"), []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", bin)

	code, _, stderr := runArgs("--root", root, "--tag", "v0.2.2", "--tag-at-head")
	if _, err := os.Stat(marker); err == nil {
		t.Fatalf("the repository's git ran (exit %d, stderr %q)", code, stderr)
	}
	if code != 2 || !strings.Contains(stderr, "inside the repository") {
		t.Fatalf("exit %d, stderr %q; want could-not-run naming the refusal", code, stderr)
	}
}

// Under `go -C` the working directory is the module, so a relative --root
// would inspect backend/go_orchestrator and report "missing package.json" —
// a mismatch for the wrong reason. It is a usage error instead.
func TestARelativeRootIsAUsageError(t *testing.T) {
	code, _, stderr := runArgs("--root", ".")
	if code != 2 || !strings.Contains(stderr, "--root must be an absolute path") {
		t.Fatalf("exit %d, stderr %q", code, stderr)
	}
}

const artifactsOutside = "FAIL: --artifacts is outside the checkout\n"

func plantArchives(t *testing.T, dir string) {
	t.Helper()
	for _, name := range []string{
		"devmap-cli-x86_64-unknown-linux-gnu.tar.xz",
		"devmap-cli-aarch64-apple-darwin.tar.xz",
		"devmap-cli-x86_64-apple-darwin.tar.xz",
		"devmap-cli-x86_64-pc-windows-msvc.zip",
	} {
		if err := os.WriteFile(filepath.Join(dir, name), []byte("x"), 0o644); err != nil {
			t.Fatal(err)
		}
	}
}

func mkdirAll(t *testing.T, dir string) string {
	t.Helper()
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	return dir
}

func symlink(t *testing.T, target, link string) {
	t.Helper()
	if err := os.Symlink(target, link); err != nil {
		t.Skipf("symlinks unavailable: %v", err)
	}
}

func assertArtifactsRefused(t *testing.T, root, artifacts string) {
	t.Helper()
	code, stdout, stderr := runArgs("--root", root, "--artifacts", artifacts)
	if code != 1 || stdout != "" || stderr != artifactsOutside {
		t.Fatalf("--artifacts %q: exit %d\nstdout %q\nstderr %q", artifacts, code, stdout, stderr)
	}
}

// An inside path is not a containment failure: it reaches Inspect, which
// fails on this empty root for its own reason.
func assertArtifactsInspected(t *testing.T, root, artifacts string) {
	t.Helper()
	code, stdout, stderr := runArgs("--root", root, "--artifacts", artifacts)
	if code != 1 || strings.Contains(stdout, "OK:") || strings.Contains(stderr, "outside the checkout") || !strings.Contains(stderr, "missing package.json") {
		t.Fatalf("--artifacts %q: exit %d\nstdout %q\nstderr %q", artifacts, code, stdout, stderr)
	}
}

func TestArtifactsThatLeaveTheCheckoutAreRefused(t *testing.T) {
	parent := t.TempDir()
	root := mkdirAll(t, filepath.Join(parent, "repo"))
	evil := mkdirAll(t, filepath.Join(parent, "repo-evil"))
	// A complete archive set outside would otherwise satisfy the check.
	plantArchives(t, evil)
	sep := string(filepath.Separator)
	for _, artifacts := range []string{
		"..",
		filepath.Join("..", "repo-evil"),
		"sub" + sep + ".." + sep + ".." + sep + "repo-evil",
		evil,
		root + sep + ".." + sep + "repo-evil",
		parent,
		filepath.Join(parent, "no-such-outside"),
	} {
		assertArtifactsRefused(t, root, artifacts)
	}
}

func TestArtifactsInsideTheCheckoutAreInspected(t *testing.T) {
	root := t.TempDir()
	mkdirAll(t, filepath.Join(root, "sub", "dir"))
	for _, artifacts := range []string{
		".",
		filepath.Join("sub", "dir"),
		filepath.Join("sub", "..", "sub", "dir"),
		filepath.Join("sub", "missing"),
		root,
		filepath.Join(root, "sub", "dir"),
	} {
		assertArtifactsInspected(t, root, artifacts)
	}
}

func TestSymlinkedArtifactsThatLeaveTheCheckoutAreRefused(t *testing.T) {
	root := t.TempDir()
	outside := t.TempDir()
	plantArchives(t, outside)
	symlink(t, outside, filepath.Join(root, "arts"))
	symlink(t, filepath.Join(outside, "not-created"), filepath.Join(root, "dangling"))
	symlink(t, filepath.Join("..", "escaped"), filepath.Join(root, "relout"))
	for _, artifacts := range []string{
		"arts",
		filepath.Join(root, "arts"),
		filepath.Join("arts", "missing"),
		"dangling",
		"relout",
	} {
		assertArtifactsRefused(t, root, artifacts)
	}

	// A link that stays inside is the checkout's own directory.
	inside := mkdirAll(t, filepath.Join(root, "inside"))
	symlink(t, inside, filepath.Join(root, "kept"))
	assertArtifactsInspected(t, root, "kept")
}

func TestASymlinkCycleIsACouldNotRunNotOK(t *testing.T) {
	root := t.TempDir()
	symlink(t, "cycle", filepath.Join(root, "cycle"))
	code, stdout, stderr := runArgs("--root", root, "--artifacts", "cycle")
	if code != 2 || stdout != "" || !strings.HasPrefix(stderr, "dcrelease:") {
		t.Fatalf("exit %d\nstdout %q\nstderr %q", code, stdout, stderr)
	}
}

// A root that is itself a symlink must not make its real path look outside,
// nor hide a path that leaves it.
func TestARootThatIsASymlinkIsComparedByItsRealPath(t *testing.T) {
	parent := t.TempDir()
	real := mkdirAll(t, filepath.Join(parent, "real"))
	outside := mkdirAll(t, filepath.Join(parent, "outside"))
	plantArchives(t, outside)
	rootLink := filepath.Join(parent, "rootlink")
	symlink(t, real, rootLink)
	assertArtifactsInspected(t, rootLink, filepath.Join(real, "sub"))
	assertArtifactsInspected(t, rootLink, filepath.Join(rootLink, "sub"))
	assertArtifactsInspected(t, rootLink, "sub")
	assertArtifactsRefused(t, rootLink, filepath.Join("..", "outside"))
	assertArtifactsRefused(t, rootLink, outside)
}

func TestAdversarialSymlinkShapesNeverAdmitOutside(t *testing.T) {
	parent := t.TempDir()
	root := mkdirAll(t, filepath.Join(parent, "repo"))
	mkdirAll(t, filepath.Join(parent, "repo2"))
	outside := mkdirAll(t, filepath.Join(parent, "outside"))
	plantArchives(t, outside)
	symlink(t, outside, filepath.Join(root, "hop2"))
	symlink(t, "hop2", filepath.Join(root, "hop1"))
	symlink(t, "hop1", filepath.Join(root, "hop0"))
	symlink(t, "..", filepath.Join(root, "up"))
	symlink(t, filepath.Join(parent, "repo2"), filepath.Join(root, "sib"))
	symlink(t, "b", filepath.Join(root, "a"))
	symlink(t, "a", filepath.Join(root, "b"))
	symlink(t, outside, filepath.Join(root, "arts"))
	sep := string(filepath.Separator)
	for _, artifacts := range []string{
		"hop0",
		"hop0" + sep + "missing",
		"up",
		"up" + sep + "outside",
		"up" + sep + "repo2",
		"sib",
		"arts" + sep,
		"." + sep + "arts",
		"arts" + sep + sep + "x",
		"arts" + sep + "." + sep + "x",
		root + sep + sep + "arts",
		filepath.Join(root, "arts") + sep,
	} {
		assertArtifactsRefused(t, root, artifacts)
	}
	code, stdout, stderr := runArgs("--root", root, "--artifacts", "a")
	if code != 2 || stdout != "" || !strings.HasPrefix(stderr, "dcrelease:") {
		t.Fatalf("two-link cycle: exit %d\nstdout %q\nstderr %q", code, stdout, stderr)
	}
}

func TestAdversarialEmptyFileAndRootSpellings(t *testing.T) {
	parent := t.TempDir()
	root := mkdirAll(t, filepath.Join(parent, "repo"))
	if err := os.WriteFile(filepath.Join(root, "afile"), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(parent, "ofile"), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	code, stdout, stderr := runArgs("--root", root, "--artifacts", "")
	if code != 2 || stdout != "" || !strings.Contains(stderr, "--artifacts needs a value") {
		t.Fatalf("empty --artifacts: exit %d\nstdout %q\nstderr %q", code, stdout, stderr)
	}
	assertArtifactsRefused(t, root, filepath.Join("..", "ofile"))
	assertArtifactsRefused(t, root, filepath.Join(parent, "ofile"))
	assertArtifactsInspected(t, root, "afile")
	assertArtifactsInspected(t, root, root+string(filepath.Separator))
	assertArtifactsInspected(t, root, "."+string(filepath.Separator))
	// Prefix confusion: root-evil and root2 share root's prefix.
	for _, name := range []string{"repo-evil", "repo2", "repository"} {
		mkdirAll(t, filepath.Join(parent, name))
		assertArtifactsRefused(t, root, filepath.Join(parent, name))
		assertArtifactsRefused(t, root, filepath.Join("..", name))
	}
}

// A spelling that differs from the checkout only in case or Unicode
// normalisation may be refused (a limitation on APFS) but never admitted
// when it names a directory outside, and never prints OK.
func TestAdversarialCaseAndNormalisationNeverAdmitOutside(t *testing.T) {
	parent := t.TempDir()
	nfc := "caf\u00e9"
	nfd := "cafe\u0301"
	root := mkdirAll(t, filepath.Join(parent, nfc))
	outside := mkdirAll(t, filepath.Join(parent, nfc+"-evil"))
	plantArchives(t, outside)
	for _, artifacts := range []string{
		filepath.Join(parent, nfd+"-evil"),
		filepath.Join(parent, "CAF\u00c9-EVIL"),
		filepath.Join(parent, nfd, "..", nfd+"-evil"),
	} {
		code, stdout, stderr := runArgs("--root", root, "--artifacts", artifacts)
		if strings.Contains(stdout, "OK:") || code == 0 || strings.Contains(stderr, "missing package.json") {
			t.Fatalf("--artifacts %q admitted: exit %d\nstdout %q\nstderr %q", artifacts, code, stdout, stderr)
		}
	}
}

// --artifacts is judged before any git runs, so a refusal prints nothing to
// stdout: no "would publish" OK line beside the FAIL.
func TestArtifactsRefusalWithTagAtHeadPrintsNoOKLine(t *testing.T) {
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
	code, stdout, stderr := runArgs("--root", dir, "--tag", "v9.9.9", "--tag-at-head", "--artifacts", "..")
	if code != 1 || stdout != "" || stderr != artifactsOutside {
		t.Fatalf("exit %d\nstdout %q\nstderr %q", code, stdout, stderr)
	}
}

// A dangling link whose target walks back through a link that leaves: the
// ".." belongs to the link's real directory, not to the lexical path.
func TestDanglingLinkThroughAnOutsideLinkIsRefused(t *testing.T) {
	parent := t.TempDir()
	root := mkdirAll(t, filepath.Join(parent, "repo"))
	outside := mkdirAll(t, filepath.Join(parent, "outside"))
	mkdirAll(t, filepath.Join(outside, "sub"))
	symlink(t, filepath.Join(outside, "sub"), filepath.Join(root, "s"))
	// Join would clean the target to "x"; the link must keep the ".." as written.
	symlink(t, "s"+string(filepath.Separator)+".."+string(filepath.Separator)+"x", filepath.Join(root, "link"))
	assertArtifactsRefused(t, root, "link")
}
