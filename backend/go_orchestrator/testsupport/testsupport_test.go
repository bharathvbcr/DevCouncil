package testsupport

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"testing"
)

// recordingTB stands in for a *testing.T so that a helper's failure path can be
// observed instead of suffered. Unavailable, Tool and RepoRoot all report
// trouble by failing the test that called them, which is precisely what a test
// of that behaviour cannot afford to let happen.
//
// testing.TB cannot be implemented from outside the testing package — it has an
// unexported method — so a TB is embedded to satisfy the interface, and left
// nil deliberately. Helper, Fatalf and Skipf are overridden below; any other TB
// method a helper reaches instead (Fatal, Skip, FailNow, Errorf) dereferences
// that nil and brings the run down. A gate that stopped being observable must
// not report the same result as one that was observed and held.
//
// Fatalf and Skipf end the goroutine, because testing's own do: each is a log
// followed by runtime.Goexit. The helpers rely on that and would behave
// differently under a recorder that returned — RepoRoot's walk would spin
// forever once its Fatalf came back, and Tool would hand the caller the empty
// string instead of stopping it. Modelling the control flow wrongly would mean
// testing a package that does not exist.
type recordingTB struct {
	testing.TB
	fatals   []string
	skips    []string
	returned bool
}

func (r *recordingTB) Helper() {}

func (r *recordingTB) Fatalf(format string, args ...any) {
	r.fatals = append(r.fatals, fmt.Sprintf(format, args...))
	runtime.Goexit()
}

func (r *recordingTB) Skipf(format string, args ...any) {
	r.skips = append(r.skips, fmt.Sprintf(format, args...))
	runtime.Goexit()
}

// probe runs one helper against a recordingTB and reports what it did to it.
//
// On its own goroutine, because Fatalf and Skipf Goexit: on the test's own
// goroutine that would end the test rather than record anything. Closing the
// channel is what orders the recorder's writes before the caller's reads, and
// it runs on the Goexit path too, so a helper that never returns is still
// waited for exactly once.
func probe(t *testing.T, call func(testing.TB)) *recordingTB {
	t.Helper()
	rec := &recordingTB{}
	done := make(chan struct{})
	go func() {
		defer close(done)
		call(rec)
		rec.returned = true
	}()
	<-done
	return rec
}

// wantFatal asserts the helper failed its TB once and did not skip or return.
func (r *recordingTB) wantFatal(t *testing.T, what string) {
	t.Helper()
	switch {
	case len(r.skips) != 0:
		t.Fatalf("%s: skipped instead of failing: %q\n"+
			"a prerequisite that was never checked would report what a checked one does",
			what, r.skips)
	case r.returned:
		t.Fatalf("%s: returned to its caller having neither failed nor skipped; "+
			"the caller carries on without the thing it asked for", what)
	case len(r.fatals) != 1:
		t.Fatalf("%s: failed %d times, want exactly 1", what, len(r.fatals))
	}
}

// wantSkip asserts the helper skipped its TB once and did not fail or return.
func (r *recordingTB) wantSkip(t *testing.T, what string) {
	t.Helper()
	switch {
	case len(r.fatals) != 0:
		t.Fatalf("%s: failed instead of skipping: %q", what, r.fatals)
	case r.returned:
		t.Fatalf("%s: returned to its caller instead of skipping it", what)
	case len(r.skips) != 1:
		t.Fatalf("%s: skipped %d times, want exactly 1", what, len(r.skips))
	}
}

// setAllowSkip puts the opt-in variable into the state a case describes and
// leaves the run's own value restored afterwards. t.Setenv is called even when
// the case wants the variable gone, because it is what registers the restore;
// Unsetenv alone would leak the absence into every test that follows.
func setAllowSkip(t *testing.T, value string, set bool) {
	t.Helper()
	t.Setenv(AllowSkipEnv, value)
	if !set {
		if err := os.Unsetenv(AllowSkipEnv); err != nil {
			t.Fatalf("unset %s: %v", AllowSkipEnv, err)
		}
	}
}

// TestUnavailableSkipsOnlyWhenTheOperatorOptsIn pins the rule that keeps a
// missing toolchain from turning a run green: Unavailable is a hard failure by
// default, and a skip only when an operator has set AllowSkipEnv to exactly 1.
//
// Both directions are asserted and neither is sufficient alone. The recorded
// cases show which way the gate goes for each value of the variable; the last
// subtest, on a real *testing.T, shows that a skip actually stops the caller,
// which is the one thing a recorder cannot demonstrate about itself.
func TestUnavailableSkipsOnlyWhenTheOperatorOptsIn(t *testing.T) {
	for _, tc := range []struct {
		name     string
		value    string
		set      bool
		wantSkip bool
	}{
		// The direction that matters. Nothing opted this run in, so a seam that
		// could not be checked has to be a failure, not a pass.
		{name: "unset", wantSkip: false},
		// Set, but not opted in. The gate is an explicit "1" rather than
		// truthiness: an operator who wrote 0 said no, and one who wrote true
		// said something this harness does not accept. Both must fail.
		{name: "empty", value: "", set: true, wantSkip: false},
		{name: "zero", value: "0", set: true, wantSkip: false},
		{name: "true", value: "true", set: true, wantSkip: false},
		// The opt-in itself.
		{name: "one", value: "1", set: true, wantSkip: true},
	} {
		t.Run(tc.name, func(t *testing.T) {
			setAllowSkip(t, tc.value, tc.set)

			rec := probe(t, func(tb testing.TB) { Unavailable(tb, "missing %s", "tool") })
			what := fmt.Sprintf("%s=%q (set=%t)", AllowSkipEnv, tc.value, tc.set)
			if tc.wantSkip {
				rec.wantSkip(t, what)
			} else {
				rec.wantFatal(t, what)
			}
		})
	}

	// The skip has to end the test, not merely be reported. This is the stash's
	// original form of the assertion: if Unavailable returns here, the Fatal
	// below runs and this subtest fails.
	t.Run("the skip stops the caller", func(t *testing.T) {
		t.Setenv(AllowSkipEnv, "1")
		Unavailable(t, "missing %s", "tool")
		t.Fatal("Unavailable returned to its caller instead of skipping it; " +
			"a test that keeps running past an unmet prerequisite asserts nothing")
	})
}

func TestToolFindsAPresentBinary(t *testing.T) {
	got := Tool(t, "go")
	if got == "" {
		t.Fatal("Tool returned an empty path for go")
	}
	if _, err := os.Stat(got); err != nil {
		t.Fatalf("Tool(%q) is not a file: %v", got, err)
	}
}

// TestToolIsUnavailableWhenTheBinaryIsMissing covers the half of Tool that
// decides whether a run without the toolchain fails or passes. Without it, Tool
// can lose its error check entirely and still look tested: the present-binary
// case above never takes the error branch.
func TestToolIsUnavailableWhenTheBinaryIsMissing(t *testing.T) {
	// A name containing a separator is tried directly instead of against PATH,
	// so this one is absent whatever the machine happens to have installed.
	absent := filepath.Join(t.TempDir(), "not-a-real-tool")

	t.Run("fails by default", func(t *testing.T) {
		setAllowSkip(t, "", false)
		rec := probe(t, func(tb testing.TB) { Tool(tb, absent) })
		rec.wantFatal(t, "Tool on a missing binary")
	})

	t.Run("skips when the operator opted in", func(t *testing.T) {
		setAllowSkip(t, "1", true)
		rec := probe(t, func(tb testing.TB) { Tool(tb, absent) })
		rec.wantSkip(t, "Tool on a missing binary, opted in")
	})
}

// TestRepoRootFindsTheRustWorkspace is the guard on the defect described at the
// top of this package: the store tests reached the Rust workspace by counting
// ".." levels, were wrong by one, and so skipped everything while printing ok.
//
// RepoRoot's contract is that cargo can be run from the workspace under what it
// returns, so that is what is checked — against the two markers the package
// accepts, restated here rather than shared with it. Both are accepted because
// DevCouncil holds the crates in rust/ and Manvi keeps only symlinks to them
// under crates/. (This sentence used to say "the same two markers cargoBin
// resolves cmd.Dir with". cargoBin resolves nothing now — it asks
// RustWorkspace — so that was a comment describing machinery deleted under it.)
//
// The restatement is deliberate; do not replace it with whatever list the
// package walks. Asserting against the code under test is how a check stops
// being able to fail — rename a marker and it renames on both sides at once.
//
// Measured at package scope on this tree, because the earlier version of this
// paragraph claimed "reddens this test and only this test" for each marker and
// that was measured under `-run TestRepoRoot`, which hid everything else:
//
//	rust   -> rustMUT:   3 fail — this test, plus
//	                     TestTheExportedBinariesNameRealCargoTargets and
//	                     TestBuiltBinaryIsNotAPathCargoRewrites
//	crates -> cratesMUT: 1 fail — TestRepoRootFindsTheManviLayout
//
// So the two markers are not symmetric. crates/ has exactly one guard, and it is
// the one below; folding that one into a shared list would drop the marker's only
// cover. rust/ has three, but only this test is an assertion about the marker —
// the other two are navigation sites that fatal incidentally once the workspace
// cannot be found, so they would fall silent together with everything else if the
// list were shared.
func TestRepoRootFindsTheRustWorkspace(t *testing.T) {
	root := RepoRoot(t)

	for _, dir := range []string{"rust", "crates"} {
		if _, err := os.Stat(filepath.Join(root, dir, "Cargo.toml")); err == nil {
			return
		}
	}
	t.Fatalf("RepoRoot = %s holds neither rust/Cargo.toml nor crates/Cargo.toml; "+
		"cargoBin runs cargo from one of those and would build nothing", root)
}

// TestRepoRootFailsWhenThereIsNoWorkspaceAbove covers the end of the walk. A
// RepoRoot that ran out of parents and returned something anyway would send
// every caller's cargo invocation to a directory with no workspace in it, and
// the failure would be reported as a broken build rather than a lost repository.
func TestRepoRootFailsWhenThereIsNoWorkspaceAbove(t *testing.T) {
	// The walk starts at the working directory. t.TempDir is called first so
	// that its removal is cleaned up after t.Chdir has put the process back.
	outside := t.TempDir()
	t.Chdir(outside)

	rec := probe(t, func(tb testing.TB) { RepoRoot(tb) })
	rec.wantFatal(t, fmt.Sprintf("RepoRoot from %s, which has no Cargo.toml above it", outside))
}

// TestRepoRootFindsTheManviLayout covers the crates/ half of the walk against a
// tree built for it, because this repository is the rust/ one and no other test
// in the suite can take that branch. A RepoRoot that quietly stopped accepting
// crates/ would stay green here and fail every test in Manvi, which is the
// asymmetry that makes a synthetic tree worth the trouble.
//
// It also exercises the walking itself: the working directory is several levels
// below the marker, which is the arrangement the hand-counted ".." this package
// replaced got wrong.
//
// The literal "crates" below is deliberate and load-bearing; do not read it from
// whatever list the package walks. This is the only test that covers that
// marker, and the fixture it plants is what makes the coverage real: renaming
// the marker in RepoRoot reddens this test as written, whereas a version that
// built its fixture from the package's own list would plant the renamed
// directory, find it, and pass — leaving the branch guarded by nothing. That is
// the whole difference between a test and a restatement of the code.
func TestRepoRootFindsTheManviLayout(t *testing.T) {
	root := t.TempDir()
	workspace := filepath.Join(root, "crates")
	if err := os.MkdirAll(workspace, 0o755); err != nil {
		t.Fatalf("build the crates/ layout: %v", err)
	}
	if err := os.WriteFile(filepath.Join(workspace, "Cargo.toml"), []byte("[workspace]\n"), 0o644); err != nil {
		t.Fatalf("write the workspace manifest: %v", err)
	}
	// No rust/Cargo.toml anywhere: the crates/ branch is reached only by failing
	// to find one, so planting both would prove nothing about this half.
	deep := filepath.Join(root, "backend", "go_orchestrator", "testsupport")
	if err := os.MkdirAll(deep, 0o755); err != nil {
		t.Fatalf("build the nested working directory: %v", err)
	}
	t.Chdir(deep)

	got := RepoRoot(t)

	// Compared through EvalSymlinks because a temp directory on darwin is handed
	// out as /var/... and read back from the working directory as /private/var/...
	// — the same directory under two names, which a string compare would call a
	// failure.
	want, err := filepath.EvalSymlinks(root)
	if err != nil {
		t.Fatalf("resolve the expected root: %v", err)
	}
	resolved, err := filepath.EvalSymlinks(got)
	if err != nil {
		t.Fatalf("resolve RepoRoot's answer %s: %v", got, err)
	}
	if resolved != want {
		t.Fatalf("RepoRoot from %s returned %s, want %s; the walk did not stop at the "+
			"directory holding crates/Cargo.toml", deep, resolved, want)
	}
}

// TestTheExportedBinariesNameRealCargoTargets checks the three (crate, binary)
// pairs DCStore, DCVerify and DCGrep hand to cargoBin against the workspace.
//
// It reads the pairs from cargo rather than from the helpers, because reading
// them from the helpers means building them: each is a one-line call whose only
// failure mode is a name cargo does not recognise, and paying a cold cargo build
// per run in *this* package to observe that is a worse trade than restating the
// pairs here. What it catches is a crate renamed in the workspace, for all three
// at once and without building any of them.
//
// `cargo metadata --no-deps` reads the workspace manifests and does not build
// or resolve dependencies; --offline makes a cargo that wanted the network say
// so instead of hanging.
func TestTheExportedBinariesNameRealCargoTargets(t *testing.T) {
	Tool(t, "cargo")

	root := RepoRoot(t)
	workspace := filepath.Join(root, "rust")
	if _, err := os.Stat(filepath.Join(workspace, "Cargo.toml")); err != nil {
		workspace = filepath.Join(root, "crates")
	}

	cmd := exec.Command("cargo", "metadata", "--no-deps", "--format-version", "1", "--offline")
	cmd.Dir = workspace
	out, err := cmd.Output()
	if err != nil {
		var exit *exec.ExitError
		if errors.As(err, &exit) {
			t.Fatalf("cargo metadata in %s: %v\n%s", workspace, err, exit.Stderr)
		}
		t.Fatalf("cargo metadata in %s: %v", workspace, err)
	}

	var meta struct {
		Packages []struct {
			Name    string `json:"name"`
			Targets []struct {
				Name string   `json:"name"`
				Kind []string `json:"kind"`
			} `json:"targets"`
		} `json:"packages"`
	}
	if err := json.Unmarshal(out, &meta); err != nil {
		t.Fatalf("parse cargo metadata from %s: %v", workspace, err)
	}

	bins := map[string]bool{}
	for _, pkg := range meta.Packages {
		for _, target := range pkg.Targets {
			for _, kind := range target.Kind {
				if kind == "bin" {
					bins[pkg.Name+" "+target.Name] = true
				}
			}
		}
	}
	if len(bins) == 0 {
		t.Fatalf("cargo metadata in %s listed no bin targets at all; "+
			"this test would pass vacuously, so it fails instead", workspace)
	}

	for _, want := range []struct{ helper, crate, binary string }{
		{"DCStore", "dc-store", "dcstore"},
		{"DCVerify", "dc-verify", "dcverify"},
		{"DCGrep", "dc-grep", "dcgrep"},
	} {
		if !bins[want.crate+" "+want.binary] {
			t.Errorf("%s asks cargoBin for -p %s --bin %s, and %s has no such bin target; "+
				"every caller of %s would fail in cargo",
				want.helper, want.crate, want.binary, workspace, want.helper)
		}
	}
}

// TestBuiltBinaryIsNotAPathCargoRewrites is the defect this package's third
// rule exists for.
//
// `go test ./...` runs dc/store and devcouncil in separate processes against
// one target directory. Whichever reaches cargo second unlinks and rewrites
// target/debug/dcstore while the first is exec'ing it, and that test dies with
// a fork/exec ENOENT naming a binary it never touched. The per-process
// sync.Once in cargoBin cannot order two processes, and cargo's workspace lock
// is released before the artifact stops moving.
//
// The rebuild loop here stands in for the other package's process, and it is
// deliberately *not* a stale build: cargo re-does its uplift even when it
// compiles nothing, so a no-op build is enough to replace the file. That is
// the case a full test run actually hits.
//
// What is asserted is that the file does not move, not that some exec happened
// to survive. Racing the exec directly would be the honest-looking test and the
// useless one: the window between cargo's unlink and its write is tens of
// microseconds, so a loop of a few hundred exec attempts misses a *present*
// regression almost every time. Measured against the pre-fix helper, such a
// loop passed three runs out of three while a tight stat loop caught the same
// build replacing the binary hundreds of times. So the test asserts the
// property that makes exec safe — the path handed to a test is one cargo has
// no reason to touch — which either holds or does not.
func TestBuiltBinaryIsNotAPathCargoRewrites(t *testing.T) {
	bin := DCStore(t)
	root := RepoRoot(t)
	crates := filepath.Join(root, "rust")
	if _, err := os.Stat(filepath.Join(crates, "Cargo.toml")); err != nil {
		crates = filepath.Join(root, "crates")
	}

	before, err := os.Stat(bin)
	if err != nil {
		t.Fatalf("stat the binary cargoBin handed out: %v", err)
	}

	for round := 0; round < 4; round++ {
		cmd := exec.Command("cargo", "build", "-p", "dc-store", "--bin", "dcstore")
		cmd.Dir = crates
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("round %d: cargo build: %v\n%s", round, err, out)
		}

		after, err := os.Stat(bin)
		if err != nil {
			t.Fatalf("round %d: a cargo build removed the binary cargoBin handed out: %v\n"+
				"tests must be given a path cargo does not own", round, err)
		}
		if !os.SameFile(before, after) {
			t.Fatalf("round %d: a cargo build replaced the binary cargoBin handed out (%s)\n"+
				"the file is a different one than the test was given; another package's "+
				"build would have done this mid-exec", round, bin)
		}
	}

	// The invariant is about the file; this is the consequence that matters.
	if err := exec.Command(bin, "--db", filepath.Join(t.TempDir(), "s.sqlite"), "health").Run(); err != nil {
		var exit *exec.ExitError
		if !errors.As(err, &exit) {
			t.Fatalf("after four concurrent-style rebuilds the binary no longer execs: %v", err)
		}
	}
}
