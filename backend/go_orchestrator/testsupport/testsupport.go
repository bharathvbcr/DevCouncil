// Package testsupport locates the repository's build outputs for tests that
// drive real binaries across the Go/Rust boundary.
//
// It exists because of a defect it is designed to make impossible. The store
// tests located the Rust workspace with a hand-counted relative path and
// skipped when it was absent. The path was wrong by one level, so every test in
// that package skipped, and `go test ./...` printed `ok` for a package that had
// executed nothing. A seam that is never exercised reported the same result as
// a seam that passed — the exact failure mode the harness's own policy layer
// exists to prevent, reproduced in its test suite.
//
// Three rules follow, and all are enforced here rather than left to each test:
//
//   - The workspace is found by walking up for a marker, never by counting
//     "..". A test file that moves between directories keeps working.
//   - A missing toolchain fails the test by default. Skipping is available only
//     when an operator opts in explicitly with MANVI_TEST_ALLOW_SKIP=1,
//     which makes an uncovered seam a deliberate, visible choice.
//   - A test never execs a path cargo owns. `go test ./...` gives each package
//     its own process, all of them sharing one target directory, and cargo
//     unlinks and rewrites its output on every build — so the binary a test is
//     exec'ing can vanish because a *different* package rebuilt it. cargoBin
//     hands out a stable copy instead. See its comment for the measurements.
//
// devcouncil: allow-unwired
//
// Every importer of this package is a `_test.go` file, and `unwired_candidates`
// discounts test importers on purpose — a test importing a module says nothing
// about whether production wired it. That rule is right, and this package is
// the case it cannot judge: being imported only by tests is not a symptom
// here, it is the entire job.
//
// The declaration is also the only way to state the half no single-repository
// analyzer can see. Manvi keeps symlinks to these crates and drives the same
// helpers from its own tree — `TestRepoRootFindsTheManviLayout` in this
// package's tests exists because a change that stayed green here once failed
// every test in Manvi. So "nothing in DevCouncil depends on this" is true and
// is not evidence that the package is stranded.
package testsupport

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"sync"
	"testing"
	"time"
)

// AllowSkipEnv opts a run into skipping when a toolchain is missing.
const AllowSkipEnv = "MANVI_TEST_ALLOW_SKIP"

// These helpers take testing.TB rather than *testing.T because the seams they
// locate are driven from fuzz targets as well as from tests, and a *testing.F
// cannot be passed as a *testing.T. Nothing here uses an API outside TB.

// Unavailable reports a missing prerequisite: a hard failure by default, a skip
// only when the operator has said an uncovered seam is acceptable for this run.
func Unavailable(t testing.TB, format string, args ...any) {
	t.Helper()
	if os.Getenv(AllowSkipEnv) == "1" {
		t.Skipf(format+" (skipping because "+AllowSkipEnv+"=1)", args...)
		return
	}
	t.Fatalf(format+"\n\nThis test drives a real binary and cannot verify anything without it. "+
		"Set "+AllowSkipEnv+"=1 to skip instead of failing, accepting that this seam goes uncovered.", args...)
}

// workspaceDirs names the directories a Rust workspace manifest may sit in.
// Canonical sources live in DevCouncil's `rust/`; Manvi keeps only symlinks to
// them under `crates/`. Spelled once so `RepoRoot` and `RustWorkspace` cannot
// come to disagree about which layouts exist.
//
// Navigation may share this list; an assertion must not. A test checking where
// the workspace lives restates the markers itself — one that iterates this
// variable would follow a rename into agreeing with it, which is the same
// tautology as reading `Hosts` back to assert something about `Hosts`.
var workspaceDirs = []string{"rust", "crates"}

// findWorkspace walks up from the working directory to the repository root and
// returns it together with the workspace directory inside it.
//
// One walk answers both questions, so there is no second lookup that could
// fail after the first succeeded — the old `RustWorkspace` joined "rust", and
// on a miss returned `root/crates` without checking it, which was correct only
// by the unstated invariant that `RepoRoot` had just accepted one of the two.
func findWorkspace(t testing.TB) (root, workspace string) {
	t.Helper()
	dir, err := os.Getwd()
	if err != nil {
		t.Fatalf("getwd: %v", err)
	}
	for {
		for _, name := range workspaceDirs {
			candidate := filepath.Join(dir, name)
			if _, err := os.Stat(filepath.Join(candidate, "Cargo.toml")); err == nil {
				return dir, candidate
			}
		}
		parent := filepath.Dir(dir)
		if parent == dir {
			t.Fatalf("no repository root above %s (looking for %v each holding Cargo.toml)", dir, workspaceDirs)
		}
		dir = parent
	}
}

// RepoRoot returns the directory that holds the Rust workspace used to build
// dcstore/dcverify/dcgrep/devmap.
func RepoRoot(t testing.TB) string {
	t.Helper()
	root, _ := findWorkspace(t)
	return root
}

// RustWorkspace returns the directory holding the Rust workspace manifest.
//
// A caller that needs a path *inside* the workspace — a crate's source, its
// target directory — asks here rather than joining "rust" itself, so the two
// layouts cannot drift apart one caller at a time. The directory returned is
// one whose `Cargo.toml` was just stat'd, never a path assembled on the
// assumption that it must be there.
func RustWorkspace(t testing.TB) string {
	t.Helper()
	_, workspace := findWorkspace(t)
	return workspace
}

type build struct {
	once sync.Once
	path string
	err  error
	log  []byte
}

var builds = map[string]*build{}
var buildsMu sync.Mutex

// DCStore builds the Rust store binary and returns its path.
func DCStore(t testing.TB) string { return cargoBin(t, "dc-store", "dcstore") }

// DCVerify builds the Rust verifier binary and returns its path.
func DCVerify(t testing.TB) string { return cargoBin(t, "dc-verify", "dcverify") }

// DCGrep builds the Rust searcher binary and returns its path.
func DCGrep(t testing.TB) string { return cargoBin(t, "dc-grep", "dcgrep") }

// cargoBin builds one binary and returns the path to a stable copy of it.
//
// The copy is the point. `cargo build` does not leave its output alone: every
// invocation re-does the uplift into target/debug, unlinking the existing file
// and writing a fresh one, *including* invocations that compile nothing because
// the build is already fresh. Measured on this workspace, twenty-five no-op
// builds left target/debug/dcstore absent for hundreds of observations of a
// tight stat loop, and its inode changed on every one.
//
// That matters because `go test ./...` runs each package in its own process.
// dc/store and devcouncil both reach cargoBin concurrently, and the sync.Once
// below is per-process, so it cannot order them. One package's no-op build
// therefore unlinks the binary another package's test is exec'ing at that
// instant, and the test fails with a fork/exec ENOENT that has nothing to do
// with what it was testing. sync.Once cannot fix this and neither can building
// more carefully: the artifact is shared, so the fix is to stop exec'ing the
// shared path.
//
// So the build output is copied once to a content-addressed path that cargo
// has no reason to touch, and that copy is what every test execs. Naming the
// copy after its own hash means concurrent processes converge on the same file
// with the same bytes without having to agree on a run identity, and publishing
// it by rename means nobody can exec it half-written.
//
// These copies live under target/, so `cargo clean` removes them with
// everything else. They are not pruned during a run: a copy another test
// process is about to exec is indistinguishable from a leftover, and deleting
// the wrong one would reintroduce exactly the failure this exists to prevent.
func cargoBin(t testing.TB, crate, binary string) string {
	t.Helper()
	// Resolved before the sync.Once, not inside it: a caller that arrives
	// second skips the Do entirely, so a root check placed in there would
	// pass for every caller but the first.
	crates := RustWorkspace(t)

	buildsMu.Lock()
	b, ok := builds[binary]
	if !ok {
		b = &build{}
		builds[binary] = b
	}
	buildsMu.Unlock()

	b.once.Do(func() {
		target := filepath.Join(crates, "target")
		if b.err = os.MkdirAll(target, 0o755); b.err != nil {
			return
		}

		// Held across the build *and* the copy. Cargo's workspace lock covers
		// only the compile and is released while target/debug is still being
		// rewritten, which is the window that produces the ENOENT.
		unlock, err := lockBuildOutputs(filepath.Join(target, buildLockName))
		if err != nil {
			b.err = fmt.Errorf("locking cargo build outputs: %w", err)
			return
		}
		defer unlock()

		cmd := exec.Command("cargo", "build", "-p", crate, "--bin", binary)
		cmd.Dir = crates
		b.log, b.err = cmd.CombinedOutput()
		if b.err != nil {
			return
		}

		built := filepath.Join(target, "debug", binary)
		data, err := readStableArtifact(built)
		if err != nil {
			b.err = err
			return
		}
		b.path, b.err = publishArtifact(target, binary, data)
	})
	if b.err != nil {
		Unavailable(t, "cannot build %s: %v\n%s", binary, b.err, b.log)
	}
	return b.path
}

// buildLockName is the lock file guarding the build-and-copy critical section.
const buildLockName = ".manvi-testbin.lock"

// testBinDir holds the stable copies, under target/ so `cargo clean` reaches
// them.
const testBinDir = "manvi-testbin"

const (
	artifactAttempts   = 50
	artifactRetryDelay = 20 * time.Millisecond
)

// readStableArtifact reads a cargo output that another process may be replacing
// underneath it, and returns only a copy it can show is whole.
//
// Two things can go wrong while reading target/debug/<binary>. The file can be
// absent, because cargo unlinks before it writes. Or it can be short, because
// cargo is still writing the replacement. The first is an error to retry. The
// second is the dangerous one: it reads as a successful read of a truncated
// binary, so it is checked for rather than assumed away. The size comes from
// the open descriptor, not the path, so it describes the same file the bytes
// came from even after the name has been reused.
func readStableArtifact(path string) ([]byte, error) {
	var last error
	for attempt := 0; attempt < artifactAttempts; attempt++ {
		if attempt > 0 {
			time.Sleep(artifactRetryDelay)
		}
		f, err := os.Open(path)
		if err != nil {
			last = err
			continue
		}
		data, readErr := io.ReadAll(f)
		info, statErr := f.Stat()
		closeErr := f.Close()
		switch {
		case readErr != nil:
			last = readErr
		case statErr != nil:
			last = statErr
		case int64(len(data)) != info.Size():
			last = fmt.Errorf("read %d bytes of %s but it is %d bytes: it is being rewritten",
				len(data), path, info.Size())
		case len(data) == 0:
			last = fmt.Errorf("%s is empty", path)
		case closeErr != nil:
			// Last, because the three above describe the read and this one
			// only describes letting go of it. Still a retry rather than a
			// pass: a close that fails on a file cargo may be replacing under
			// us is evidence the bytes just read came from a file that was
			// moving, which is the condition this loop exists to survive.
			last = closeErr
		default:
			return data, nil
		}
	}
	return nil, fmt.Errorf("could not read a complete %s in %d attempts: %w",
		path, artifactAttempts, last)
}

// publishArtifact writes the bytes to a path named after their own hash and
// returns it, creating the file by rename so no reader ever sees a partial one.
// An entry that already exists is left alone: its name is its content, so it
// cannot differ from what we would write.
func publishArtifact(target, binary string, data []byte) (string, error) {
	sum := sha256.Sum256(data)
	dir := filepath.Join(target, testBinDir, binary+"-"+hex.EncodeToString(sum[:])[:16])
	path := filepath.Join(dir, binary)
	if info, err := os.Stat(path); err == nil && info.Size() == int64(len(data)) {
		return path, nil
	}
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return "", err
	}
	tmp, err := os.CreateTemp(dir, binary+".partial-*")
	if err != nil {
		return "", err
	}
	// Best-effort: the success path renames this away, so a failure here means
	// the rename already took it. Named rather than dropped so the next reader
	// does not have to work out which.
	defer func() { _ = os.Remove(tmp.Name()) }()
	if _, err := tmp.Write(data); err != nil {
		_ = tmp.Close()
		return "", err
	}
	if err := tmp.Close(); err != nil {
		return "", err
	}
	if err := os.Chmod(tmp.Name(), 0o755); err != nil {
		return "", err
	}
	if err := os.Rename(tmp.Name(), path); err != nil {
		return "", err
	}
	return path, nil
}

// Tool checks that an external command the test depends on exists.
func Tool(t testing.TB, name string) string {
	t.Helper()
	path, err := exec.LookPath(name)
	if err != nil {
		Unavailable(t, "%s is not on PATH: %v", name, err)
	}
	return path
}
