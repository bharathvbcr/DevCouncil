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
	t.Setenv(dcverify.BinaryEnv, "")

	if client := verify.RigorClient(t.TempDir()); client != nil {
		t.Fatalf("RigorClient invented a verifier: %+v", client)
	}
}
