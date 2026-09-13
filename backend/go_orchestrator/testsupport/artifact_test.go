package testsupport

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// The two functions exercised here are the whole mechanism behind this
// package's third rule, and both were reached only through a cargo build before
// these tests existed: a run that could not build got no coverage of them at
// all, and a run that could covered barely half. They are pure enough to drive
// directly, so they are driven directly.

func TestReadStableArtifactReturnsTheWholeFile(t *testing.T) {
	path := filepath.Join(t.TempDir(), "dcstore")
	want := bytes.Repeat([]byte{0x7f, 'E', 'L', 'F', 0x02}, 4096)
	if err := os.WriteFile(path, want, 0o755); err != nil {
		t.Fatalf("stage the artifact: %v", err)
	}

	got, err := readStableArtifact(path)
	if err != nil {
		t.Fatalf("readStableArtifact(%s): %v", path, err)
	}
	if !bytes.Equal(got, want) {
		t.Fatalf("read %d bytes, want the %d that were written", len(got), len(want))
	}
}

// TestReadStableArtifactWaitsForAFileCargoHasNotWrittenYet covers the retry the
// function exists for: cargo unlinks before it writes, so a read can begin
// while the path is absent and must still come back with the finished bytes.
//
// The writer lands at 60ms against a budget of artifactAttempts *
// artifactRetryDelay, which is a full second — a sixteen-fold margin. The
// assertion is on the bytes, never on how many attempts it took, so a slow
// machine makes the read take longer rather than making the test wrong.
func TestReadStableArtifactWaitsForAFileCargoHasNotWrittenYet(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "dcstore")
	want := []byte("the finished artifact")

	staged := make(chan error, 1)
	go func() {
		time.Sleep(60 * time.Millisecond)
		// Written elsewhere and renamed in, the way publishArtifact does, so
		// that what is being asserted is the retry and not a torn read.
		tmp := filepath.Join(dir, "staging")
		if err := os.WriteFile(tmp, want, 0o755); err != nil {
			staged <- err
			return
		}
		staged <- os.Rename(tmp, path)
	}()

	got, err := readStableArtifact(path)
	if stageErr := <-staged; stageErr != nil {
		t.Fatalf("staging the artifact: %v", stageErr)
	}
	if err != nil {
		t.Fatalf("readStableArtifact gave up on a file that appeared 60ms in: %v", err)
	}
	if !bytes.Equal(got, want) {
		t.Fatalf("read %q, want %q", got, want)
	}
}

// TestReadStableArtifactNeverAcceptsAnEmptyFile covers the branch that stops a
// zero-byte read from being handed out as a binary. Cargo's unlink-then-write
// leaves an empty file visible, and os.ReadFile reports that as a clean success.
func TestReadStableArtifactNeverAcceptsAnEmptyFile(t *testing.T) {
	path := filepath.Join(t.TempDir(), "dcstore")
	if err := os.WriteFile(path, nil, 0o755); err != nil {
		t.Fatalf("stage the empty file: %v", err)
	}

	got, err := readStableArtifact(path)
	if err == nil {
		t.Fatalf("readStableArtifact accepted a zero-byte file and returned %d bytes; "+
			"the caller would publish it and every test would exec an empty binary", len(got))
	}
	if !strings.Contains(err.Error(), "is empty") {
		t.Fatalf("the error does not say the file was empty, so a reader cannot tell this "+
			"from a missing file: %v", err)
	}
}

// TestReadStableArtifactGivesUpWithTheReasonItKeptFailing covers the exhausted
// loop. Giving up is correct; giving up without the cause is what turns a
// missing checkout into an unexplained "could not read".
func TestReadStableArtifactGivesUpWithTheReasonItKeptFailing(t *testing.T) {
	path := filepath.Join(t.TempDir(), "never-written")

	got, err := readStableArtifact(path)
	if err == nil {
		t.Fatalf("readStableArtifact produced %d bytes for a file that does not exist", len(got))
	}
	if !strings.Contains(err.Error(), path) {
		t.Errorf("the error does not name the file it could not read: %v", err)
	}
	if !strings.Contains(err.Error(), "attempts") {
		t.Errorf("the error does not say it retried, so a reader cannot tell a slow "+
			"build from an absent one: %v", err)
	}
	if !errors.Is(err, os.ErrNotExist) {
		t.Errorf("the error does not unwrap to the cause that kept failing: %v", err)
	}
}

func TestPublishArtifactNamesTheCopyAfterItsContents(t *testing.T) {
	target := t.TempDir()
	data := []byte("#!/bin/sh\necho dcstore\n")

	path, err := publishArtifact(target, "dcstore", data)
	if err != nil {
		t.Fatalf("publishArtifact: %v", err)
	}

	sum := sha256.Sum256(data)
	want := filepath.Join(target, testBinDir, "dcstore-"+hex.EncodeToString(sum[:])[:16], "dcstore")
	if path != want {
		t.Fatalf("published to\n  %s\nwant the content-addressed\n  %s\n"+
			"the name is what lets concurrent processes converge without agreeing "+
			"on a run identity", path, want)
	}

	got, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read the published copy: %v", err)
	}
	if !bytes.Equal(got, data) {
		t.Fatalf("the published copy holds %q, want %q", got, data)
	}

	info, err := os.Stat(path)
	if err != nil {
		t.Fatalf("stat the published copy: %v", err)
	}
	if info.Mode().Perm()&0o111 == 0 {
		t.Fatalf("the published copy is not executable (mode %v); every caller execs it",
			info.Mode())
	}
}

// TestPublishArtifactIsContentAddressed pins both halves of what the naming
// buys: the same bytes reach the same path, and reaching it again leaves the
// file that is already there alone. The second half is the one that matters at
// run time — replacing a file another test process is exec'ing is the failure
// this whole mechanism exists to prevent.
func TestPublishArtifactIsContentAddressed(t *testing.T) {
	target := t.TempDir()
	data := []byte("the first artifact")

	first, err := publishArtifact(target, "dcstore", data)
	if err != nil {
		t.Fatalf("publishArtifact: %v", err)
	}
	before, err := os.Stat(first)
	if err != nil {
		t.Fatalf("stat the first copy: %v", err)
	}

	again, err := publishArtifact(target, "dcstore", data)
	if err != nil {
		t.Fatalf("republishing the same bytes: %v", err)
	}
	if again != first {
		t.Fatalf("the same bytes published to two paths:\n  %s\n  %s", first, again)
	}
	after, err := os.Stat(again)
	if err != nil {
		t.Fatalf("stat the republished copy: %v", err)
	}
	if !os.SameFile(before, after) {
		t.Fatalf("republishing the same bytes replaced the file at %s; "+
			"a concurrent test process exec'ing it would have lost it mid-exec", first)
	}

	other, err := publishArtifact(target, "dcstore", []byte("a different artifact"))
	if err != nil {
		t.Fatalf("publishing different bytes: %v", err)
	}
	if other == first {
		t.Fatalf("different bytes published to the same path %s; "+
			"one build's output would be handed out as another's", first)
	}
}

// TestPublishArtifactReplacesACopyOfTheWrongSize covers the size check guarding
// the early return. A copy that is already there is trusted, but only as far as
// its length: a leftover from an interrupted write has the right name, because
// the name came from the bytes that were meant to be there, not the ones that are.
func TestPublishArtifactReplacesACopyOfTheWrongSize(t *testing.T) {
	target := t.TempDir()
	data := []byte("a whole artifact")

	path, err := publishArtifact(target, "dcstore", data)
	if err != nil {
		t.Fatalf("publishArtifact: %v", err)
	}
	if err := os.WriteFile(path, data[:4], 0o755); err != nil {
		t.Fatalf("truncate the published copy: %v", err)
	}

	again, err := publishArtifact(target, "dcstore", data)
	if err != nil {
		t.Fatalf("republishing over a truncated copy: %v", err)
	}
	if again != path {
		t.Fatalf("republished to %s, want the content-addressed %s", again, path)
	}
	got, err := os.ReadFile(again)
	if err != nil {
		t.Fatalf("read the republished copy: %v", err)
	}
	if !bytes.Equal(got, data) {
		t.Fatalf("a truncated leftover was handed back as-is: %q, want %q", got, data)
	}
}

// There is deliberately no test here for publishArtifact's deferred
// os.Remove of its staging file. One was written and then removed: on the
// success path os.Rename moves that file to its final name, so nothing is left
// for the defer to delete and the assertion holds whether or not the defer
// exists — measured by deleting the defer, which left the test green. The
// staging file can only survive if Write, Close, Chmod or Rename fails after
// CreateTemp succeeded, and none of those can be made to fail deterministically
// from outside the package. A green test that cannot go red is the thing this
// package exists to prevent, so it is not kept.

// TestPublishArtifactReportsADirectoryItCannotCreate covers the MkdirAll error
// path. Returning a path it had not written would be the worst of the failures
// available here: cargoBin would hand a caller a name with no file behind it.
func TestPublishArtifactReportsADirectoryItCannotCreate(t *testing.T) {
	// A regular file standing where the directory tree has to go. MkdirAll
	// cannot pass through it, on any platform.
	blocked := filepath.Join(t.TempDir(), "target")
	if err := os.WriteFile(blocked, nil, 0o644); err != nil {
		t.Fatalf("stage the blocking file: %v", err)
	}

	path, err := publishArtifact(blocked, "dcstore", []byte("some bytes"))
	if err == nil {
		t.Fatalf("publishArtifact returned %s instead of reporting that it could not "+
			"create its directory", path)
	}
	if path != "" {
		t.Errorf("publishArtifact returned both an error and the path %q", path)
	}
}
