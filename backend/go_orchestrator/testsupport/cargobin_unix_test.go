//go:build unix

package testsupport

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"
)

// These tests drive cargoBin against a workspace whose `cargo` is a script that
// does not finish until the test lets it, so a build that outlasts its bound is
// something the test arranges rather than something it hopes a slow machine
// provides. Each one gives the helper a deadline a couple of seconds past
// testDeadlineMargin and asserts it failed — not skipped, not hung — well
// before that deadline would have passed.
//
// Each asks for its own binary name, because builds is keyed by it and a name
// shared with another test would hand this one that test's finished build.

// probeLimit is how long a probe waits before concluding the helper is not
// bounded at all. The deadlines below leave the helper about two seconds.
const probeLimit = 20 * time.Second

// deadlineTB is a recordingTB that also answers Deadline, the way *testing.T
// does.
type deadlineTB struct {
	*recordingTB
	deadline time.Time
}

func (d *deadlineTB) Deadline() (time.Time, bool) { return d.deadline, true }

// withDeadline returns a recorder whose test deadline is `after` from now.
func withDeadline(after time.Duration) (*recordingTB, *deadlineTB) {
	rec := &recordingTB{}
	return rec, &deadlineTB{recordingTB: rec, deadline: time.Now().Add(after)}
}

// probeWithin is probe with a limit: it reports false if call is still running
// after limit, which is the outcome an unbounded wait produces. The goroutine
// is then left behind, and rec must not be read.
func probeWithin(limit time.Duration, rec *recordingTB, call func()) bool {
	done := make(chan struct{})
	go func() {
		defer close(done)
		call()
		rec.returned = true
	}()
	select {
	case <-done:
		return true
	case <-time.After(limit):
		return false
	}
}

// fakeWorkspace makes a Rust workspace, puts it in the working directory, and
// puts a `cargo` first on PATH that says it is waiting, the way the slot gate
// does, records that it started, and then runs until release is called (or the
// test ends). It returns the workspace directory and the path the script
// touches when it starts. After succeedFrom(workspace) is created, the script
// instead writes the requested binary and exits 0, as a finished build would.
func fakeWorkspace(t *testing.T) (workspace, started string, release func()) {
	t.Helper()
	root := t.TempDir()
	workspace = filepath.Join(root, "rust")
	bin := filepath.Join(root, "bin")
	for _, dir := range []string{workspace, bin} {
		if err := os.MkdirAll(dir, 0o755); err != nil {
			t.Fatal(err)
		}
	}
	if err := os.WriteFile(filepath.Join(workspace, "Cargo.toml"), []byte("[workspace]\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	hold := filepath.Join(root, "hold")
	if err := os.WriteFile(hold, nil, 0o644); err != nil {
		t.Fatal(err)
	}
	started = filepath.Join(root, "started")
	script := fmt.Sprintf("#!/bin/sh\necho '%s' >&2\n: > '%s'\n"+
		"if [ -e '%s' ]; then mkdir -p target/debug && printf built > \"target/debug/$5\" && exit 0; fi\n"+
		"while [ -e '%s' ]; do sleep 0.1; done\nexit 1\n", fakeCargoSays, started, succeedFrom(workspace), hold)
	if err := os.WriteFile(filepath.Join(bin, "cargo"), []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", bin+string(os.PathListSeparator)+os.Getenv("PATH"))
	t.Chdir(root)
	release = func() { _ = os.Remove(hold) }
	t.Cleanup(release)
	return workspace, started, release
}

// fakeCargoSays is what the fake cargo prints before it waits.
const fakeCargoSays = "fake cargo: waiting for a build slot"

// succeedFrom is the file that turns the fake cargo into one that succeeds.
func succeedFrom(workspace string) string { return filepath.Join(filepath.Dir(workspace), "succeed") }

// TestCargoBinGivesUpBeforeTheTestDeadline is the defect seen from Manvi: the
// build was bounded only by cargoBuildBound (20 minutes), so a slow or queued
// build outlived go test's 10-minute alarm, which panicked with a goroutine
// dump that looked like a hang. The build must instead stop before the test's
// own deadline and fail naming the build.
//
// The opt-in to skip is set on purpose: running out of time is not a missing
// toolchain, and must fail even for an operator who accepts uncovered seams.
func TestCargoBinGivesUpBeforeTheTestDeadline(t *testing.T) {
	_, started, _ := fakeWorkspace(t)
	setAllowSkip(t, "1", true)
	rec, tb := withDeadline(testDeadlineMargin + 2*time.Second)

	begin := time.Now()
	if !probeWithin(probeLimit, rec, func() { cargoBin(tb, "fake-crate", "fake-deadline") }) {
		t.Fatalf("cargoBin was still building after %s, when its deadline left it about 2s; "+
			"the build is not bounded by the test's deadline, so go test's alarm would report it as a goroutine dump",
			probeLimit)
	}
	if _, err := os.Stat(started); err != nil {
		t.Fatalf("the fake cargo never ran, so nothing here exercised the build: %v", err)
	}
	rec.wantFatal(t, "cargoBin with a build that outlasts the test deadline")
	if elapsed := time.Since(begin); elapsed > 10*time.Second {
		t.Errorf("cargoBin gave up after %s; it had about 2s before its margin", elapsed)
	}
	t.Logf("the failure a test sees:\n%s", rec.fatals[0])
	// The last is cargo's own output: once the killed build is reaped, it is
	// the evidence of where the time went, and must not be dropped.
	for _, want := range []string{"cargo build -p fake-crate --bin fake-deadline", "-timeout deadline", errOverran.Error(), fakeCargoSays} {
		if !strings.Contains(rec.fatals[0], want) {
			t.Errorf("the failure does not say %q:\n%s", want, rec.fatals[0])
		}
	}
}

// TestCargoBinGivesUpOnAContendedLockBeforeTheTestDeadline covers the other
// unbounded wait: the build-output lock was taken with a blocking flock, so a
// test process holding it through its own long build held every other process
// past its deadline. A second open file description contends exactly as
// another `go test` process would.
func TestCargoBinGivesUpOnAContendedLockBeforeTheTestDeadline(t *testing.T) {
	workspace, started, _ := fakeWorkspace(t)
	setAllowSkip(t, "", false)
	target := filepath.Join(workspace, "target")
	if err := os.MkdirAll(target, 0o755); err != nil {
		t.Fatal(err)
	}
	lockPath := filepath.Join(target, buildLockName)
	holder, err := os.OpenFile(lockPath, os.O_CREATE|os.O_RDWR, 0o600)
	if err != nil {
		t.Fatal(err)
	}
	if err := syscall.Flock(int(holder.Fd()), syscall.LOCK_EX); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		_ = syscall.Flock(int(holder.Fd()), syscall.LOCK_UN)
		_ = holder.Close()
	})
	rec, tb := withDeadline(testDeadlineMargin + 2*time.Second)

	if !probeWithin(probeLimit, rec, func() { cargoBin(tb, "fake-crate", "fake-lock") }) {
		t.Fatalf("cargoBin was still waiting for a held build lock after %s, when its deadline left it about 2s; "+
			"the lock wait is not bounded", probeLimit)
	}
	rec.wantFatal(t, "cargoBin while another process holds the build lock")
	for _, want := range []string{"build lock", lockPath, errOverran.Error()} {
		if !strings.Contains(rec.fatals[0], want) {
			t.Errorf("the failure does not say %q:\n%s", want, rec.fatals[0])
		}
	}
	if _, err := os.Stat(started); err == nil {
		t.Error("cargo ran while another process held the build lock")
	}
}

// TestCargoBinRefusesToStartWithNoTimeLeft: a test whose deadline is already
// inside the margin fails at once, rather than launching a build only to kill
// it, or running one the alarm will interrupt.
func TestCargoBinRefusesToStartWithNoTimeLeft(t *testing.T) {
	_, started, _ := fakeWorkspace(t)
	setAllowSkip(t, "", false)
	rec, tb := withDeadline(testDeadlineMargin / 2)

	if !probeWithin(probeLimit, rec, func() { cargoBin(tb, "fake-crate", "fake-no-time") }) {
		t.Fatalf("cargoBin was still building after %s with no time left before the test's deadline", probeLimit)
	}
	rec.wantFatal(t, "cargoBin with the deadline inside the margin")
	if !strings.Contains(rec.fatals[0], "no time is left") {
		t.Errorf("the failure does not say no time was left:\n%s", rec.fatals[0])
	}
	if _, err := os.Stat(started); err == nil {
		t.Error("cargo was started although there was no time left to run it")
	}
}

// TestASecondCallerWaitsUnderItsOwnDeadline: callers in one process share one
// build. One that arrives while another's build is running used to wait inside
// sync.Once with no bound of its own — the first caller's bound, or none for a
// caller without a deadline, such as a fuzz target. It must give up at its own.
func TestASecondCallerWaitsUnderItsOwnDeadline(t *testing.T) {
	_, started, release := fakeWorkspace(t)
	setAllowSkip(t, "", false)

	// The first caller has no deadline, so its build is bounded only by the cap.
	first := &recordingTB{}
	firstDone := make(chan struct{})
	go func() {
		defer close(firstDone)
		cargoBin(first, "fake-crate", "fake-join")
	}()
	for wait := time.Now(); ; time.Sleep(20 * time.Millisecond) {
		if _, err := os.Stat(started); err == nil {
			break
		}
		if time.Since(wait) > 10*time.Second {
			release()
			t.Fatal("the first caller's build never started")
		}
	}

	rec, tb := withDeadline(testDeadlineMargin + 2*time.Second)
	finished := probeWithin(probeLimit, rec, func() { cargoBin(tb, "fake-crate", "fake-join") })
	release()
	select {
	case <-firstDone:
	case <-time.After(probeLimit):
		t.Error("the first caller did not finish after its cargo exited")
	}
	if !finished {
		t.Fatalf("a second caller was still waiting on the first caller's build after %s, when its own deadline left it about 2s", probeLimit)
	}
	rec.wantFatal(t, "a second caller of a build that outlasts its deadline")
	if !strings.Contains(rec.fatals[0], "another test in this process started the build") {
		t.Errorf("the failure does not say it was waiting on another caller's build:\n%s", rec.fatals[0])
	}
}

// TestAFinishedBuildIsHandedOutWithNoTimeLeft: a caller arriving after the
// build has finished waits for nothing, so it gets the binary however close its
// deadline is. Checking the deadline first failed every such caller with "no
// time is left" — in a full run, every test in a package after the first.
func TestAFinishedBuildIsHandedOutWithNoTimeLeft(t *testing.T) {
	workspace, _, _ := fakeWorkspace(t)
	setAllowSkip(t, "", false)
	if err := os.WriteFile(succeedFrom(workspace), nil, 0o644); err != nil {
		t.Fatal(err)
	}

	first, tb := withDeadline(testDeadlineMargin + 10*time.Second)
	if !probeWithin(probeLimit, first, func() { cargoBin(tb, "fake-crate", "fake-finished") }) {
		t.Fatalf("a build that succeeds at once had not returned after %s", probeLimit)
	}
	if !first.returned {
		t.Fatalf("the first caller did not get the binary: fatals %q skips %q", first.fatals, first.skips)
	}

	late, lateTB := withDeadline(testDeadlineMargin / 2)
	var got string
	if !probeWithin(probeLimit, late, func() { got = cargoBin(lateTB, "fake-crate", "fake-finished") }) {
		t.Fatalf("a caller of a finished build had not returned after %s", probeLimit)
	}
	if !late.returned {
		t.Fatalf("a caller of a finished build was failed for want of time it did not need: %q", late.fatals)
	}
	if data, err := os.ReadFile(got); err != nil || string(data) != "built" {
		t.Fatalf("the late caller was handed %q, which does not hold the build (%q, %v)", got, data, err)
	}
}

// TestLockBuildOutputsGivesUpWhenItsContextEnds pins the lock's own bound,
// and the shape cargoBin depends on: no release function with the error.
func TestLockBuildOutputsGivesUpWhenItsContextEnds(t *testing.T) {
	path := filepath.Join(t.TempDir(), buildLockName)
	release, err := lockBuildOutputs(context.Background(), path)
	if err != nil {
		t.Fatal(err)
	}
	defer release()

	ctx, cancel := context.WithTimeout(context.Background(), 300*time.Millisecond)
	defer cancel()
	begin := time.Now()
	second, err := lockBuildOutputs(ctx, path)
	if err == nil {
		second()
		t.Fatal("a second holder took the lock while it was held")
	}
	if !errors.Is(err, errOverran) {
		t.Errorf("a wait that ran out is not reported as one: %v", err)
	}
	if second != nil {
		t.Error("lockBuildOutputs returned both an error and a release function")
	}
	if elapsed := time.Since(begin); elapsed > 5*time.Second {
		t.Errorf("the wait ended %s after a 300ms context", elapsed)
	}
}
