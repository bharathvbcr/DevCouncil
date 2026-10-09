package verify_test

// How the host finds a verifier, which is a question about trust before it is a
// question about convenience.
//
// The repository under analysis is *input*. Its contents do not authorize
// executing a program, so a build output, a vendored helper, or anything a
// relative PATH entry resolves to inside the checkout must not become the
// process that decides whether that repository's diff contains a credential. A
// diff that could choose its own scanner is a diff that passes every scan.

import (
	"os"
	"path/filepath"
	"runtime"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/dcverify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
)

// plantOnPath writes an executable named dcverify into dir and puts dir on
// PATH for the duration of the test.
func plantOnPath(t *testing.T, dir string) string {
	t.Helper()
	if runtime.GOOS == "windows" {
		t.Skip("the planted binaries here are shell scripts")
	}
	path := filepath.Join(dir, "dcverify")
	// #nosec G306 -- an executable stand-in; the execute bit is the point.
	if err := os.WriteFile(path, []byte("#!/bin/sh\nexit 0\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", dir)
	// Cleared so the override branch does not mask what PATH discovery does.
	t.Setenv(dcverify.BinaryEnv, "")
	return path
}

// TestAVerifierInsideTheRepositoryIsNotUsed. The repository is the thing being
// judged; it does not get to supply the judge.
func TestAVerifierInsideTheRepositoryIsNotUsed(t *testing.T) {
	root := t.TempDir()
	inside := filepath.Join(root, "bin")
	if err := os.MkdirAll(inside, 0o755); err != nil {
		t.Fatal(err)
	}
	plantOnPath(t, inside)

	if client := verify.RigorClient(root); client != nil {
		t.Fatalf("RigorClient resolved %q, which is inside the repository under analysis", client.Binary)
	}
}

// TestAVerifierOutsideTheRepositoryIsUsed is the positive control. Without it
// the test above would pass just as well against a function that always
// returned nil — and a rigor layer that never finds a verifier is not a safe
// rigor layer, it is an absent one.
func TestAVerifierOutsideTheRepositoryIsUsed(t *testing.T) {
	root := t.TempDir()
	planted := plantOnPath(t, t.TempDir())

	client := verify.RigorClient(root)
	if client == nil {
		t.Fatal("RigorClient found nothing for a verifier outside the repository")
	}
	// Resolved through symlinks on the way, so compare canonically rather than
	// by spelling — /var and /private/var name the same file on macOS.
	got, err := filepath.EvalSymlinks(client.Binary)
	if err != nil {
		t.Fatal(err)
	}
	want, err := filepath.EvalSymlinks(planted)
	if err != nil {
		t.Fatal(err)
	}
	if got != want {
		t.Errorf("resolved %q, want %q", got, want)
	}
	if client.Root != root {
		t.Errorf("client root=%q, want the repository %q", client.Root, root)
	}
}

// TestTheEnvironmentOverrideWins. An operator who names a path means it,
// including one inside the repository — which is how this is developed and
// tested. The override is a deliberate act by someone with a shell, not
// something repository contents can arrange.
func TestTheEnvironmentOverrideWins(t *testing.T) {
	root := t.TempDir()
	inside := filepath.Join(root, "bin", "dcverify")
	if err := os.MkdirAll(filepath.Dir(inside), 0o755); err != nil {
		t.Fatal(err)
	}
	// #nosec G306 -- an executable stand-in; the execute bit is the point.
	if err := os.WriteFile(inside, []byte("#!/bin/sh\nexit 0\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv(dcverify.BinaryEnv, inside)

	client := verify.RigorClient(root)
	if client == nil || client.Binary != inside {
		t.Fatalf("the override was not honoured: %+v", client)
	}
}

// TestNoVerifierAnywhereIsNilRatherThanAGuess. A Client whose Binary is the
// bare name "dcverify" would fail at exec time once per verify, with a message
// about a path. Nil produces one sentence naming what to install — and this is
// the common state, since dcverify is an optional component.
func TestNoVerifierAnywhereIsNilRatherThanAGuess(t *testing.T) {
	t.Setenv("PATH", t.TempDir())
	// ~/.local/bin is a rung of component discovery, so an absent
	// component is absent there too.
	t.Setenv("HOME", t.TempDir())
	t.Setenv(dcverify.BinaryEnv, "")

	if client := verify.RigorClient(t.TempDir()); client != nil {
		t.Fatalf("RigorClient invented a verifier: %+v", client)
	}
}

// TestAMissingOverrideIsStillTheVerifierThatRuns. A named verifier that does
// not exist is a configured verifier that is broken: running it fails as a
// blocking gap. Returning nil would turn the operator's misconfiguration into
// a skip that reads like dcverify was never installed.
func TestAMissingOverrideIsStillTheVerifierThatRuns(t *testing.T) {
	missing := filepath.Join(t.TempDir(), "no-such-dcverify")
	t.Setenv(dcverify.BinaryEnv, missing)

	client := verify.RigorClient(t.TempDir())
	if client == nil || client.Binary != missing {
		t.Fatalf("a broken override was not kept as the verifier: %+v", client)
	}
}

// TestARepositoryLocalGitDoesNotDescribeTheDiff. The diff verification
// judges comes from git; a git the repository supplies could report any diff
// it likes, including none.
func TestARepositoryLocalGitDoesNotDescribeTheDiff(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("the planted git is a shell script")
	}
	root := t.TempDir()
	marker := filepath.Join(t.TempDir(), "ran")
	if err := os.MkdirAll(filepath.Join(root, "bin"), 0o755); err != nil {
		t.Fatal(err)
	}
	// #nosec G306 -- an executable stand-in; the execute bit is the point.
	if err := os.WriteFile(filepath.Join(root, "bin", "git"), []byte("#!/bin/sh\n: > \"$PLANTED_GIT_MARKER\"\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PLANTED_GIT_MARKER", marker)
	t.Setenv("PATH", filepath.Join(root, "bin"))

	_, _, _, err := verify.CollectGitState(t.Context(), root)
	if _, statErr := os.Stat(marker); statErr == nil {
		t.Fatal("the repository's git ran")
	}
	if err == nil {
		t.Fatal("a refused git produced a diff instead of an error")
	}
}

// TestARepositoryLinkToAnOutsideProgramIsStillTheRepositorysChoice. The
// repository does not have to ship the program to choose it: a link it
// carries named git and pointing at /usr/bin/true is a git that exits 0 with
// no output, which reads as "nothing changed". The binding is repository
// content even when the bytes are not.
func TestARepositoryLinkToAnOutsideProgramIsStillTheRepositorysChoice(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("symlinks to /usr/bin/true are a unix fixture")
	}
	truePath := "/usr/bin/true"
	if _, err := os.Stat(truePath); err != nil {
		t.Skipf("no %s here", truePath)
	}
	root := t.TempDir()
	if err := os.MkdirAll(filepath.Join(root, "bin"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(truePath, filepath.Join(root, "bin", "git")); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "changed.go"), []byte("package x\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", filepath.Join(root, "bin"))

	changed, diff, empty, err := verify.CollectGitState(t.Context(), root)
	if err == nil {
		t.Fatalf("a repository-chosen git described the tree: changed=%v diff=%q empty=%v", changed, diff, empty)
	}
}
