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
//     when an operator opts in explicitly with DEVCOUNCIL_TEST_ALLOW_SKIP=1,
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
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
)

// AllowSkipEnv opts a run into skipping when a toolchain is missing.
const AllowSkipEnv = "DEVCOUNCIL_TEST_ALLOW_SKIP"

// retiredAllowSkipEnv is the name AllowSkipEnv had while these helpers lived in
// Manvi. It no longer opts anything in, and it is read only so that a config
// still setting it fails naming its replacement. Honouring it would let a stale
// CI file keep seams uncovered under a name nobody chose for this repository;
// ignoring it would turn that run red with a message that says nothing about
// the rename. Manvi's verify.sh refuses both names for the same reason.
const retiredAllowSkipEnv = "MANVI_TEST_ALLOW_SKIP"

// These helpers take testing.TB rather than *testing.T because the seams they
// locate are driven from fuzz targets as well as from tests, and a *testing.F
// cannot be passed as a *testing.T. Nothing here uses an API outside TB.

// Unavailable reports a missing prerequisite: a hard failure by default, a skip
// only when the operator has said an uncovered seam is acceptable for this run.
func Unavailable(t testing.TB, format string, args ...any) {
	t.Helper()
	// Checked before the opt-in, so a config that migrated but left the old
	// line behind still hears about it.
	if _, set := os.LookupEnv(retiredAllowSkipEnv); set {
		t.Fatalf(format+"\n\n"+retiredAllowSkipEnv+" is set, but it was renamed to "+AllowSkipEnv+
			" and no longer opts a run into skipping. Remove it, and set "+AllowSkipEnv+
			"=1 if this seam may go uncovered.", args...)
		return
	}
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

// build is one binary's build, shared by every caller in this process. The
// first caller starts it; every caller, the first included, waits for done
// under its own deadline. path, err and log are written before done closes and
// read only after it has.
type build struct {
	start sync.Once
	done  chan struct{}
	path  string
	err   error
	log   []byte
}

var builds = map[string]*build{}
var buildsMu sync.Mutex

// errOverran marks a build that ran out of time: the lock wait, the cargo
// build, or another caller's build this one was waiting on. It fails the test
// even when AllowSkipEnv is set, because running out of time says nothing
// about whether the toolchain is present. Letting it skip would turn a slow
// machine into an uncovered seam that reports like a deliberate one.
var errOverran = errors.New("the test build ran out of time")

// DCStore builds the Rust store binary and returns its path.
func DCStore(t testing.TB) string {
	t.Helper()
	return cargoBin(t, "dc-store", "dcstore")
}

// TaskSeed describes one task to plant in a store: a workbench item of the
// store's repository. Zero values are a ready task titled after its ID with an
// empty scope.
type TaskSeed struct {
	ID     string
	Status string
	Title  string
	// Scope is the item's scope object as JSON; empty means {}.
	Scope string
	// Priority is 1 (highest) to 3; zero leaves the default.
	Priority int
}

// dcstoreBound caps one seeding call to the store binary, and
// dcstoreOutputLimit how much of its reply is kept.
const (
	dcstoreBound       = 2 * time.Minute
	dcstoreOutputLimit = 1 << 20
)

// runDCStore runs the store binary once under a deadline with a bounded reply.
// A timeout, a non-zero exit or a reply cut at the limit fails the test: a
// truncated reply would otherwise be parsed as if it were whole.
func runDCStore(t testing.TB, bin string, args ...string) []byte {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), dcstoreBound)
	defer cancel()
	cmd := exec.CommandContext(ctx, bin, args...)
	proc.ConfigureGroup(cmd)
	out := &proc.CappedBuffer{Limit: dcstoreOutputLimit}
	cmd.Stdout, cmd.Stderr = out, out
	cmd.WaitDelay = 5 * time.Second
	err, timedOut := proc.RunBounded(ctx, cmd.Run)
	if timedOut {
		// The abandoned goroutine may still be writing out; it is not read.
		t.Fatalf("dcstore %s did not return within %s", strings.Join(args, " "), dcstoreBound)
	}
	if err != nil {
		t.Fatalf("dcstore %s: %v\n%s", strings.Join(args, " "), err, out.Bytes())
	}
	if out.Overflowed() {
		t.Fatalf("dcstore %s replied with more than %d bytes", strings.Join(args, " "), dcstoreOutputLimit)
	}
	return out.Bytes()
}

// SeedTask plants a task the way an operator does, through `dcstore work`, so
// the test exercises the one definition of a task rather than a private copy
// of its storage. The store is created if it does not exist. A refusal fails
// the test with the store's own message.
func SeedTask(t testing.TB, db string, seed TaskSeed) {
	t.Helper()
	bin := DCStore(t)
	run := func(args ...string) string {
		return string(runDCStore(t, bin, append([]string{"--db", db}, args...)...))
	}
	run("local-repository")
	if seed.Status == "" {
		seed.Status = "ready"
	}
	if seed.Title == "" {
		seed.Title = seed.ID
	}
	if seed.Scope == "" {
		seed.Scope = "{}"
	}
	priority := ""
	if seed.Priority != 0 {
		priority = fmt.Sprintf(`,"priority":%d`, seed.Priority)
	}
	title, _ := json.Marshal(seed.Title)
	input := fmt.Sprintf(`{"request_id":"seed-%s","id":%q,"expected_revision":0,"title":%s,"status":%q%s,`+
		`"repository_ids":["this-repo"],"primary_repository_id":"this-repo","scope":%s}`,
		seed.ID, seed.ID, title, seed.Status, priority, seed.Scope)
	if out := run("work", "--method", "items.put", "--input", input); !strings.Contains(out, `"ok":true`) {
		t.Fatalf("seeding task %s was refused: %s", seed.ID, out)
	}
}

// MergeTaskScope sets the named keys of an existing task's scope, keeping the
// rest, through the same revisioned `items.put` an edit takes. It exists
// because a task's body cannot be edited with the sqlite3 CLI: the item search
// index is an FTS5 table that the system CLI may not be built with, and a
// direct write would skip the scope validation anyway.
func MergeTaskScope(t testing.TB, db, id string, patch map[string]any) {
	t.Helper()
	bin := DCStore(t)
	run := func(method string, input []byte) []byte {
		return runDCStore(t, bin, "--db", db, "work", "--method", method, "--input", string(input))
	}
	request, _ := json.Marshal(map[string]string{"id": id})
	var got struct {
		Item struct {
			Title    string         `json:"title"`
			Status   string         `json:"status"`
			Revision int64          `json:"revision"`
			Scope    map[string]any `json:"scope"`
			Repos    []string       `json:"repository_ids"`
			Primary  string         `json:"primary_repository_id"`
		} `json:"item"`
	}
	if err := json.Unmarshal(run("items.get", request), &got); err != nil || got.Item.Revision == 0 {
		t.Fatalf("reading task %s: %v", id, err)
	}
	scope := got.Item.Scope
	if scope == nil {
		scope = map[string]any{}
	}
	for key, value := range patch {
		scope[key] = value
	}
	put, err := json.Marshal(map[string]any{
		"request_id": fmt.Sprintf("scope-%s-%d", id, got.Item.Revision), "id": id,
		"expected_revision": got.Item.Revision, "title": got.Item.Title, "status": got.Item.Status,
		"repository_ids": got.Item.Repos, "primary_repository_id": got.Item.Primary, "scope": scope,
	})
	if err != nil {
		t.Fatal(err)
	}
	if out := run("items.put", put); !strings.Contains(string(out), `"ok":true`) {
		t.Fatalf("changing the scope of %s was refused: %s", id, out)
	}
}

// DCVerify builds the Rust verifier binary and returns its path.
func DCVerify(t testing.TB) string {
	t.Helper()
	return cargoBin(t, "dc-verify", "dcverify")
}

// DCGrep builds the Rust searcher binary and returns its path.
func DCGrep(t testing.TB) string {
	t.Helper()
	return cargoBin(t, "dc-grep", "dcgrep")
}

// Devmap returns the devmap binary: DEVMAP_BIN when set — used or refused,
// never replaced, the rule the host binary applies — else a build of this
// workspace's `devmap-cli`.
func Devmap(t testing.TB) string {
	t.Helper()
	if explicit := strings.TrimSpace(os.Getenv("DEVMAP_BIN")); explicit != "" {
		info, err := os.Stat(explicit)
		if err != nil || info.IsDir() {
			t.Fatalf("DEVMAP_BIN=%s is not a binary (%v); an explicit binary is used or refused", explicit, err)
		}
		return explicit
	}
	return cargoBin(t, "devmap-cli", "devmap")
}

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
//
// Every wait in here ends before the calling test's deadline: the lock, the
// build, and another caller's build in this process. `go test` arms an alarm
// at its -timeout (ten minutes by default) that panics with every goroutine's
// stack, and a wait that outlasts it is reported as that dump, which looks
// like a hang and names nothing. Ending first, with a message naming what was
// waited for, is the difference. See buildBound for how the time is chosen.
func cargoBin(t testing.TB, crate, binary string) string {
	t.Helper()
	// Resolved before the sync.Once, not inside it: a caller that arrives
	// second skips the Do entirely, so a root check placed in there would
	// pass for every caller but the first.
	crates := RustWorkspace(t)

	buildsMu.Lock()
	b, ok := builds[binary]
	if !ok {
		b = &build{done: make(chan struct{})}
		builds[binary] = b
	}
	buildsMu.Unlock()

	// A build that has already finished costs no time, so its outcome is
	// reported before the deadline is consulted: a late test must not fail
	// for want of time to wait on something it need not wait for.
	select {
	case <-b.done:
		return b.outcome(t, binary)
	default:
	}

	bound, boundBy := buildBound(t)
	if bound <= 0 {
		t.Fatalf("cannot build %s: no time is left for it under %s", binary, boundBy)
	}
	stop := time.Now().Add(bound)

	// The build runs on its own goroutine rather than inside the Do, because
	// a second caller blocks in Do until the first caller's function returns
	// and so would wait under the first caller's bound instead of its own.
	b.start.Do(func() {
		go func() {
			defer close(b.done)
			b.path, b.log, b.err = buildAndPublish(stop, boundBy, crates, crate, binary)
		}()
	})

	// The grace covers the copy after a build that finished just in time; the
	// build itself is cut off at stop.
	wait := time.NewTimer(time.Until(stop) + publishGrace)
	defer wait.Stop()
	select {
	case <-b.done:
	case <-wait.C:
		t.Fatalf("cannot build %s: %v: another test in this process started the build, and it had not "+
			"finished within %s, the bound set by %s", binary, errOverran, bound.Round(time.Second), boundBy)
	}
	return b.outcome(t, binary)
}

// outcome hands a caller the finished build, or fails it with the reason.
// Only called once done has closed.
func (b *build) outcome(t testing.TB, binary string) string {
	t.Helper()
	if b.err != nil {
		if errors.Is(b.err, errOverran) {
			t.Fatalf("cannot build %s: %v\n%s", binary, b.err, b.log)
		}
		Unavailable(t, "cannot build %s: %v\n%s", binary, b.err, b.log)
	}
	return b.path
}

// buildAndPublish builds one binary under the lock and copies it out, giving
// up at stop. boundBy says what set stop, for the error.
func buildAndPublish(stop time.Time, boundBy, crates, crate, binary string) (string, []byte, error) {
	ctx, cancel := context.WithDeadline(context.Background(), stop)
	defer cancel()
	target := filepath.Join(crates, "target")
	if err := os.MkdirAll(target, 0o755); err != nil {
		return "", nil, err
	}

	// Held across the build *and* the copy. Cargo's workspace lock covers
	// only the compile and is released while target/debug is still being
	// rewritten, which is the window that produces the ENOENT.
	unlock, err := lockBuildOutputs(ctx, filepath.Join(target, buildLockName))
	if err != nil {
		return "", nil, fmt.Errorf("locking cargo build outputs: %w (the wait was bounded by %s)", err, boundBy)
	}
	defer unlock()

	started := time.Now()
	cmd := exec.CommandContext(ctx, "cargo", "build", "-p", crate, "--bin", binary)
	proc.ConfigureGroup(cmd)
	cmd.Dir = crates
	log := &proc.CappedBuffer{Limit: cargoLogLimit}
	cmd.Stdout, cmd.Stderr = log, log
	cmd.WaitDelay = 5 * time.Second
	// The method value, not cmd.Run(): that call is what the process-wide
	// bound test forbids, because Start only becomes cancellable after the
	// fork. The wrapper exists to close ran once that call returns.
	run := cmd.Run
	ran := make(chan struct{})
	runErr, timedOut := proc.RunBounded(ctx, func() error {
		defer close(ran)
		return run()
	})
	if timedOut {
		err := fmt.Errorf("%w: `cargo build -p %s --bin %s` in %s was stopped after %s, the bound set by %s.%s",
			errOverran, crate, binary, crates, time.Since(started).Round(time.Second), boundBy, cargoWaitHint(crate, binary))
		// The deadline has already had cargo's group killed, and WaitDelay
		// bounds the reap, so Run normally returns moments later. Once it has,
		// nothing writes log any more and it can be shown: it is the evidence
		// of where the time went — the slot gate prints that it is waiting.
		// If Run is stuck in Start, the goroutine is abandoned and log, which
		// it may still write, is not read.
		select {
		case <-ran:
			return "", log.Bytes(), err
		case <-time.After(cmd.WaitDelay + time.Second):
			return "", nil, err
		}
	}
	if runErr != nil {
		return "", log.Bytes(), runErr
	}

	data, err := readStableArtifact(filepath.Join(target, "debug", binary))
	if err != nil {
		return "", log.Bytes(), err
	}
	path, err := publishArtifact(target, binary, data)
	return path, log.Bytes(), err
}

// cargoWaitHint says where the time of a build that overran may have gone.
//
// The build's output, shown with the error when cargo could be reaped, is the
// evidence; this names the one known cause of a build that sits idle — the
// cargo-slot-gate wrapper Claude Code shells put on PATH, which queues every
// gated build behind the slots other sessions hold — when the environment says
// it is in play, and the remedies either way.
func cargoWaitHint(crate, binary string) string {
	hint := ""
	if os.Getenv("CLAUDECODE") == "1" && os.Getenv("CARGO_SLOT_HELD") == "" {
		cargo, err := exec.LookPath("cargo")
		if err != nil {
			cargo = "cargo"
		}
		hint = fmt.Sprintf(" This is a Claude Code shell, where %s may be the cargo-slot-gate wrapper: "+
			"it queues each build until one of its slots (CARGO_SLOT_DIR, default ~/.cache/cargo-slots) is free, "+
			"and the time spent queued behind other sessions' builds counts against this bound.", cargo)
	}
	return hint + fmt.Sprintf(" Re-run with a larger go test -timeout, or build it first with "+
		"`cargo build -p %s --bin %s` so this build has nothing to compile.", crate, binary)
}

// buildBound is how long cargoBin may spend on one binary, and what set it.
//
// It is the test's own deadline less testDeadlineMargin when the test has one
// that comes sooner than cargoBuildBound, and cargoBuildBound otherwise. The
// margin leaves time for the failure to be reported, and for cleanups to run,
// before go test's alarm fires. testing.TB has no Deadline; *testing.T does,
// and a *testing.F or *testing.B, or a run with -timeout 0, falls back to the
// cap.
func buildBound(t testing.TB) (time.Duration, string) {
	if d, ok := t.(interface{ Deadline() (time.Time, bool) }); ok {
		if deadline, set := d.Deadline(); set {
			if left := time.Until(deadline) - testDeadlineMargin; left < cargoBuildBound {
				return left, fmt.Sprintf("the go test -timeout deadline at %s less a %s margin",
					deadline.Format(time.TimeOnly), testDeadlineMargin)
			}
		}
	}
	return cargoBuildBound, fmt.Sprintf("the %s cap on a test build", cargoBuildBound)
}

// cargoBuildBound caps how long one `cargo build` of a test binary may take,
// and cargoLogLimit how much of its output is kept for the failure message.
// testDeadlineMargin is how far before the test's deadline cargoBin gives up,
// and publishGrace how long past its bound a caller waits for the outcome: the
// copy of a build that finished in time, or the reap of one that did not.
const (
	cargoBuildBound    = 20 * time.Minute
	cargoLogLimit      = 1 << 20
	testDeadlineMargin = 30 * time.Second
	publishGrace       = 10 * time.Second
)

// buildLockName is the lock file guarding the build-and-copy critical section.
const buildLockName = ".devcouncil-testbin.lock"

// testBinDir holds the stable copies, under target/ so `cargo clean` reaches
// them.
const testBinDir = "devcouncil-testbin"

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
