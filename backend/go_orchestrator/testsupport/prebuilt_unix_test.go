//go:build unix

package testsupport

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// These tests pin the prebuilt-binary mode: a caller that has already built
// dcstore, dcverify and dcgrep (a gate script, one gated cargo call) makes the
// helpers use those binaries with zero cargo calls, and a missing or stale one
// is refused rather than rebuilt. The variable names are spelled out as
// literals, not through the package's constants, so renaming a constant cannot
// carry its own test along with it.

// countingCargo replaces the fake workspace's cargo with one that records every
// invocation, one line each, and returns the file it records to. The count of
// lines is the assertion: "no cargo call" is a number, not an absence of output.
func countingCargo(t *testing.T, workspace string) (calls string) {
	t.Helper()
	root := filepath.Dir(workspace)
	calls = filepath.Join(root, "cargo-calls")
	script := fmt.Sprintf("#!/bin/sh\necho \"$@\" >> '%s'\nexit 1\n", calls)
	if err := os.WriteFile(filepath.Join(root, "bin", "cargo"), []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	return calls
}

func cargoCalls(t *testing.T, calls string) int {
	t.Helper()
	data, err := os.ReadFile(calls)
	if os.IsNotExist(err) {
		return 0
	}
	if err != nil {
		t.Fatal(err)
	}
	return len(strings.Split(strings.TrimSpace(string(data)), "\n"))
}

// plantSource writes a workspace source file with the given age.
func plantSource(t *testing.T, workspace, rel string, age time.Duration) string {
	t.Helper()
	path := filepath.Join(workspace, rel)
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte("fn main() {}\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	when := time.Now().Add(-age)
	if err := os.Chtimes(path, when, when); err != nil {
		t.Fatal(err)
	}
	return path
}

// plantBinary writes an executable at path with the given age.
func plantBinary(t *testing.T, path string, age time.Duration) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte("#!/bin/sh\nexit 0\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	when := time.Now().Add(-age)
	if err := os.Chtimes(path, when, when); err != nil {
		t.Fatal(err)
	}
}

// ageManifest makes the fake workspace's Cargo.toml old, as a checked-out
// manifest is, so a test decides freshness only through the sources it plants.
func ageManifest(t *testing.T, workspace string) {
	t.Helper()
	old := time.Now().Add(-3 * time.Hour)
	if err := os.Chtimes(filepath.Join(workspace, "Cargo.toml"), old, old); err != nil {
		t.Fatal(err)
	}
}

// prebuiltEnvClear removes every variable the mode reads, restoring the runner's.
func prebuiltEnvClear(t *testing.T) {
	t.Helper()
	for _, name := range []string{"DEVCOUNCIL_TEST_NO_CARGO", "DCSTORE_BIN", "DCVERIFY_BIN", "DCGREP_BIN", "DEVMAP_BIN", "CARGO_TARGET_DIR"} {
		t.Setenv(name, "")
		if err := os.Unsetenv(name); err != nil {
			t.Fatal(err)
		}
	}
}

func TestNoCargoModeUsesPrebuiltBinariesWithZeroCargoCalls(t *testing.T) {
	workspace, started, _ := fakeWorkspace(t)
	setAllowSkip(t, "", false)
	prebuiltEnvClear(t)
	ageManifest(t, workspace)
	calls := countingCargo(t, workspace)
	plantSource(t, workspace, "dc-store/src/lib.rs", time.Hour)
	for _, bin := range []string{"dcstore", "dcverify", "dcgrep"} {
		plantBinary(t, filepath.Join(workspace, "target", "debug", bin), time.Minute)
	}
	t.Setenv("DEVCOUNCIL_TEST_NO_CARGO", "1")

	for name, get := range map[string]func(testing.TB) string{"dcstore": DCStore, "dcverify": DCVerify, "dcgrep": DCGrep} {
		var got string
		rec := probe(t, func(tb testing.TB) { got = get(tb) })
		if !rec.returned {
			t.Fatalf("%s: refused a fresh prebuilt binary: fatals %q skips %q", name, rec.fatals, rec.skips)
		}
		if want := filepath.Join(workspace, "target", "debug", name); got != want {
			t.Errorf("%s: got %s, want the prebuilt %s", name, got, want)
		}
	}
	if n := cargoCalls(t, calls); n != 0 {
		t.Errorf("no-cargo mode made %d cargo calls, want 0", n)
	}
	if _, err := os.Stat(started); err == nil {
		t.Error("the slow fake cargo was started")
	}
}

func TestNoCargoModeRefusesAMissingBinaryInsteadOfBuilding(t *testing.T) {
	workspace, _, _ := fakeWorkspace(t)
	setAllowSkip(t, "1", true) // a refusal is not a missing toolchain: it must fail even so
	prebuiltEnvClear(t)
	ageManifest(t, workspace)
	calls := countingCargo(t, workspace)
	t.Setenv("DEVCOUNCIL_TEST_NO_CARGO", "1")

	rec := probe(t, func(tb testing.TB) { DCStore(tb) })
	rec.wantFatal(t, "no-cargo mode with no binary")
	for _, want := range []string{"DEVCOUNCIL_TEST_NO_CARGO", "dcstore", "never runs cargo"} {
		if !strings.Contains(rec.fatals[0], want) {
			t.Errorf("the refusal does not say %q:\n%s", want, rec.fatals[0])
		}
	}
	if n := cargoCalls(t, calls); n != 0 {
		t.Errorf("a refused binary still made %d cargo calls", n)
	}
}

func TestPrebuiltBinaryOlderThanItsSourcesIsRefused(t *testing.T) {
	for _, via := range []string{"mode", "explicit"} {
		t.Run(via, func(t *testing.T) {
			workspace, _, _ := fakeWorkspace(t)
			setAllowSkip(t, "", false)
			prebuiltEnvClear(t)
			ageManifest(t, workspace)
			calls := countingCargo(t, workspace)
			bin := filepath.Join(workspace, "target", "debug", "dcstore")
			plantBinary(t, bin, time.Hour)
			src := plantSource(t, workspace, "dc-store/src/lib.rs", time.Minute)
			if via == "mode" {
				t.Setenv("DEVCOUNCIL_TEST_NO_CARGO", "1")
			} else {
				t.Setenv("DCSTORE_BIN", bin)
			}

			rec := probe(t, func(tb testing.TB) { DCStore(tb) })
			rec.wantFatal(t, "a prebuilt binary older than its sources")
			resolved, err := filepath.EvalSymlinks(src)
			if err != nil {
				t.Fatal(err)
			}
			for _, want := range []string{"older than", resolved} {
				if !strings.Contains(rec.fatals[0], want) {
					t.Errorf("the refusal does not say %q:\n%s", want, rec.fatals[0])
				}
			}
			if n := cargoCalls(t, calls); n != 0 {
				t.Errorf("a stale binary triggered %d cargo calls instead of a refusal", n)
			}
		})
	}
}

func TestExplicitBinaryEnvIsUsedOrRefusedNeverReplaced(t *testing.T) {
	workspace, _, _ := fakeWorkspace(t)
	setAllowSkip(t, "", false)
	prebuiltEnvClear(t)
	ageManifest(t, workspace)
	calls := countingCargo(t, workspace)
	plantSource(t, workspace, "dc-verify/src/main.rs", time.Hour)
	elsewhere := filepath.Join(t.TempDir(), "dcverify")
	plantBinary(t, elsewhere, time.Minute)
	t.Setenv("DCVERIFY_BIN", elsewhere)

	var got string
	rec := probe(t, func(tb testing.TB) { got = DCVerify(tb) })
	if !rec.returned || got != elsewhere {
		t.Fatalf("DCVERIFY_BIN was not used: got %q, fatals %q", got, rec.fatals)
	}

	t.Setenv("DCVERIFY_BIN", filepath.Join(t.TempDir(), "absent"))
	probe(t, func(tb testing.TB) { DCVerify(tb) }).wantFatal(t, "an explicit binary that does not exist")
	if n := cargoCalls(t, calls); n != 0 {
		t.Errorf("an explicit binary made %d cargo calls", n)
	}
}

func TestBinaryFreshnessIgnoresTargetOutput(t *testing.T) {
	workspace, _, _ := fakeWorkspace(t)
	setAllowSkip(t, "", false)
	prebuiltEnvClear(t)
	ageManifest(t, workspace)
	countingCargo(t, workspace)
	plantBinary(t, filepath.Join(workspace, "target", "debug", "dcgrep"), time.Hour)
	// Build output newer than the binary is not a source.
	plantSource(t, workspace, "target/debug/build/gen.rs", time.Second)
	plantSource(t, workspace, "dc-grep/src/lib.rs", 2*time.Hour)
	t.Setenv("DEVCOUNCIL_TEST_NO_CARGO", "1")
	if rec := probe(t, func(tb testing.TB) { DCGrep(tb) }); !rec.returned {
		t.Fatalf("a file under target/ made the binary look stale: %q", rec.fatals)
	}
}

// Manvi's layout: crates/<name> is a symlink to a crate elsewhere, so a source
// behind the link must count.
func TestBinaryFreshnessFollowsSymlinkedCrates(t *testing.T) {
	workspace, _, _ := fakeWorkspace(t)
	setAllowSkip(t, "", false)
	prebuiltEnvClear(t)
	ageManifest(t, workspace)
	countingCargo(t, workspace)
	plantBinary(t, filepath.Join(workspace, "target", "debug", "dcgrep"), time.Hour)
	linked := filepath.Join(t.TempDir(), "dc-linked")
	plantSource(t, linked, "src/lib.rs", time.Second)
	if err := os.Symlink(linked, filepath.Join(workspace, "dc-linked")); err != nil {
		t.Fatal(err)
	}
	t.Setenv("DEVCOUNCIL_TEST_NO_CARGO", "1")

	rec := probe(t, func(tb testing.TB) { DCGrep(tb) })
	rec.wantFatal(t, "a source newer than the binary behind a symlinked crate")
	if !strings.Contains(rec.fatals[0], "lib.rs") {
		t.Errorf("the refusal does not name the newer source:\n%s", rec.fatals[0])
	}
}
