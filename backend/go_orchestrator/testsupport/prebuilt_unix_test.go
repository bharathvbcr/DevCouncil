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
// dcstore, dcverify, dcgrep and devmap (a gate script, one gated cargo call)
// makes the helpers use those binaries with zero cargo calls, and a missing or
// stale one is refused rather than rebuilt. The variable names are spelled out
// as literals, not through the package's constants, so renaming a constant
// cannot carry its own test along with it.

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

func chtimes(t *testing.T, path string, when time.Time) {
	t.Helper()
	if err := os.Chtimes(path, when, when); err != nil {
		t.Fatal(err)
	}
}

// plantSource writes a source file with the given age.
func plantSource(t *testing.T, workspace, rel string, age time.Duration) string {
	t.Helper()
	path := filepath.Join(workspace, rel)
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte("fn main() {}\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	chtimes(t, path, time.Now().Add(-age))
	return path
}

// plantBinary writes an executable with the given age, and the dep-info cargo
// would leave beside it listing sources.
func plantBinary(t *testing.T, path string, age time.Duration, sources ...string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte("#!/bin/sh\nexit 0\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	chtimes(t, path, time.Now().Add(-age))
	if sources != nil {
		plantDepInfo(t, path, strings.Join(sources, " "))
	}
}

// plantDepInfo writes <binary>.d beside the binary with the given dependency text.
func plantDepInfo(t *testing.T, binary, deps string) {
	t.Helper()
	text := binary + ": " + deps + "\n"
	if err := os.WriteFile(binary+".d", []byte(text), 0o644); err != nil {
		t.Fatal(err)
	}
}

// ageManifest makes the fake workspace's Cargo.toml old, as a checked-out
// manifest is.
func ageManifest(t *testing.T, workspace string) {
	t.Helper()
	chtimes(t, filepath.Join(workspace, "Cargo.toml"), time.Now().Add(-3*time.Hour))
}

// prebuiltSetup is a fake workspace in a clean environment with a counting
// cargo and an aged manifest.
func prebuiltSetup(t *testing.T) (workspace, calls string) {
	t.Helper()
	workspace, _, _ = fakeWorkspace(t)
	setAllowSkip(t, "", false)
	for _, name := range []string{"DEVCOUNCIL_TEST_NO_CARGO", "DCSTORE_BIN", "DCVERIFY_BIN", "DCGREP_BIN", "DEVMAP_BIN", "CARGO_TARGET_DIR"} {
		t.Setenv(name, "")
		if err := os.Unsetenv(name); err != nil {
			t.Fatal(err)
		}
	}
	ageManifest(t, workspace)
	return workspace, countingCargo(t, workspace)
}

// debugBin is where a plain cargo build leaves a binary.
func debugBin(workspace, name string) string {
	return filepath.Join(workspace, "target", "debug", name)
}

// refused runs one helper and asserts it failed, returning the message.
func refused(t *testing.T, what string, call func(testing.TB) string) string {
	t.Helper()
	rec := probe(t, func(tb testing.TB) { call(tb) })
	rec.wantFatal(t, what)
	return rec.fatals[0]
}

func TestNoCargoModeUsesPrebuiltBinariesWithZeroCargoCalls(t *testing.T) {
	workspace, calls := prebuiltSetup(t)
	src := plantSource(t, workspace, "dc-store/src/lib.rs", time.Hour)
	for _, bin := range []string{"dcstore", "dcverify", "dcgrep", "devmap"} {
		plantBinary(t, debugBin(workspace, bin), time.Minute, src)
	}
	t.Setenv("DEVCOUNCIL_TEST_NO_CARGO", "1")

	for name, get := range map[string]func(testing.TB) string{
		"dcstore": DCStore, "dcverify": DCVerify, "dcgrep": DCGrep, "devmap": Devmap,
	} {
		var got string
		rec := probe(t, func(tb testing.TB) { got = get(tb) })
		if !rec.returned {
			t.Fatalf("%s: refused a fresh prebuilt binary: fatals %q skips %q", name, rec.fatals, rec.skips)
		}
		// The rule at the top of testsupport.go: never hand out a path cargo
		// owns. The caller gets a content-addressed copy of the same bytes.
		if got == debugBin(workspace, name) || !strings.Contains(got, filepath.Join("target", testBinDir)) {
			t.Errorf("%s: handed %s, not a stable copy under target/%s", name, got, testBinDir)
		}
		want, _ := os.ReadFile(debugBin(workspace, name))
		if data, err := os.ReadFile(got); err != nil || string(data) != string(want) {
			t.Errorf("%s: the copy at %s does not hold the prebuilt bytes (%v)", name, got, err)
		}
	}
	if n := cargoCalls(t, calls); n != 0 {
		t.Errorf("no-cargo mode made %d cargo calls, want 0", n)
	}
}

func TestNoCargoModeRefusesAMissingBinaryInsteadOfBuilding(t *testing.T) {
	_, calls := prebuiltSetup(t)
	setAllowSkip(t, "1", true) // a refusal is not a missing toolchain: it must fail even so
	t.Setenv("DEVCOUNCIL_TEST_NO_CARGO", "1")

	msg := refused(t, "no-cargo mode with no binary", DCStore)
	for _, want := range []string{"DEVCOUNCIL_TEST_NO_CARGO", "dcstore", "never runs cargo"} {
		if !strings.Contains(msg, want) {
			t.Errorf("the refusal does not say %q:\n%s", want, msg)
		}
	}
	if n := cargoCalls(t, calls); n != 0 {
		t.Errorf("a refused binary still made %d cargo calls", n)
	}
}

// A file the binary was not compiled from — an editor's droppings, another
// crate, a doc — must not make a fresh binary look stale; one it was compiled
// from must.
func TestFreshnessIsJudgedByWhatTheBinaryWasBuiltFrom(t *testing.T) {
	workspace, calls := prebuiltSetup(t)
	built := plantSource(t, workspace, "dc-store/src/lib.rs", 2*time.Hour)
	plantBinary(t, debugBin(workspace, "dcstore"), time.Hour, built)
	plantSource(t, workspace, ".DS_Store", time.Second)
	plantSource(t, workspace, "dc-other/src/lib.rs", time.Second)
	t.Setenv("DEVCOUNCIL_TEST_NO_CARGO", "1")

	if rec := probe(t, func(tb testing.TB) { DCStore(tb) }); !rec.returned {
		t.Fatalf("a newer file the binary was not built from made it look stale: %q", rec.fatals)
	}

	// Now the listed source changes. The check ran before and its success is
	// remembered only for an unchanged binary, so a rebuilt-after-edit binary is
	// a different one; here the edit comes after the binary and must refuse.
	chtimes(t, debugBin(workspace, "dcstore"), time.Now().Add(-30*time.Minute))
	chtimes(t, built, time.Now().Add(-time.Minute))
	msg := refused(t, "a source listed in dep-info newer than the binary", DCStore)
	if !strings.Contains(msg, built) || !strings.Contains(msg, "not newer than") {
		t.Errorf("the refusal does not name the listed source:\n%s", msg)
	}
	if n := cargoCalls(t, calls); n != 0 {
		t.Errorf("a stale binary triggered %d cargo calls instead of a refusal", n)
	}
}

func TestEqualModificationTimeIsNotFresh(t *testing.T) {
	workspace, _ := prebuiltSetup(t)
	when := time.Now().Add(-time.Hour)
	built := plantSource(t, workspace, "dc-store/src/lib.rs", time.Hour)
	plantBinary(t, debugBin(workspace, "dcstore"), time.Hour, built)
	chtimes(t, built, when)
	chtimes(t, debugBin(workspace, "dcstore"), when)
	t.Setenv("DEVCOUNCIL_TEST_NO_CARGO", "1")
	refused(t, "a binary with the same mtime as its source", DCStore)
}

func TestManifestsNewerThanTheBinaryRefuseIt(t *testing.T) {
	for _, rel := range []string{"Cargo.toml", "Cargo.lock", "dc-store/Cargo.toml"} {
		t.Run(rel, func(t *testing.T) {
			workspace, _ := prebuiltSetup(t)
			built := plantSource(t, workspace, "dc-store/src/lib.rs", 2*time.Hour)
			plantBinary(t, debugBin(workspace, "dcstore"), time.Hour, built)
			manifest := plantSource(t, workspace, rel, time.Minute)
			t.Setenv("DEVCOUNCIL_TEST_NO_CARGO", "1")
			if msg := refused(t, "a manifest newer than the binary", DCStore); !strings.Contains(msg, manifest) {
				t.Errorf("the refusal does not name %s:\n%s", manifest, msg)
			}
		})
	}
}

func TestUnverifiableDepInfoIsRefused(t *testing.T) {
	cases := map[string]func(t *testing.T, bin, src string){
		"missing": func(t *testing.T, bin, src string) { _ = os.Remove(bin + ".d") },
		"garbage": func(t *testing.T, bin, src string) {
			plantDepInfo(t, bin, "")
			_ = os.WriteFile(bin+".d", []byte("no separator here\n"), 0o644)
		},
		"empty deps":   func(t *testing.T, bin, src string) { plantDepInfo(t, bin, "") },
		"vanished src": func(t *testing.T, bin, src string) { plantDepInfo(t, bin, src+"/gone.rs") },
	}
	for name, mutate := range cases {
		t.Run(name, func(t *testing.T) {
			workspace, calls := prebuiltSetup(t)
			built := plantSource(t, workspace, "dc-store/src/lib.rs", 2*time.Hour)
			bin := debugBin(workspace, "dcstore")
			plantBinary(t, bin, time.Hour, built)
			mutate(t, bin, filepath.Dir(built))
			t.Setenv("DEVCOUNCIL_TEST_NO_CARGO", "1")
			refused(t, "dep-info that cannot show the binary current", DCStore)
			if n := cargoCalls(t, calls); n != 0 {
				t.Errorf("made %d cargo calls", n)
			}
		})
	}
}

// A refusal must not be remembered for the process: the next test that fixes
// the cause gets the binary.
func TestARefusalIsNotCached(t *testing.T) {
	workspace, _ := prebuiltSetup(t)
	built := plantSource(t, workspace, "dc-store/src/lib.rs", 2*time.Hour)
	bin := debugBin(workspace, "dcstore")
	plantBinary(t, bin, time.Hour, built)
	_ = os.Remove(bin + ".d")
	t.Setenv("DEVCOUNCIL_TEST_NO_CARGO", "1")
	refused(t, "no dep-info", DCStore)
	plantDepInfo(t, bin, built)
	if rec := probe(t, func(tb testing.TB) { DCStore(tb) }); !rec.returned {
		t.Fatalf("a refusal was remembered after its cause was fixed: %q", rec.fatals)
	}
}

func TestExplicitBinaryEnvIsUsedOrRefusedNeverReplaced(t *testing.T) {
	workspace, calls := prebuiltSetup(t)
	built := plantSource(t, workspace, "dc-verify/src/main.rs", time.Hour)
	elsewhere := filepath.Join(t.TempDir(), "dcverify")
	plantBinary(t, elsewhere, time.Minute, built)
	t.Setenv("DCVERIFY_BIN", elsewhere)

	var got string
	rec := probe(t, func(tb testing.TB) { got = DCVerify(tb) })
	if !rec.returned {
		t.Fatalf("DCVERIFY_BIN was not used: fatals %q", rec.fatals)
	}
	if data, err := os.ReadFile(got); err != nil || string(data) != "#!/bin/sh\nexit 0\n" {
		t.Fatalf("the stable copy %s does not hold the named binary (%v)", got, err)
	}

	t.Setenv("DCVERIFY_BIN", filepath.Join(t.TempDir(), "absent"))
	refused(t, "an explicit binary that does not exist", DCVerify)
	t.Setenv("DCVERIFY_BIN", "relative/dcverify")
	if msg := refused(t, "a relative override", DCVerify); !strings.Contains(msg, "relative") {
		t.Errorf("the refusal does not say the path is relative:\n%s", msg)
	}
	if n := cargoCalls(t, calls); n != 0 {
		t.Errorf("an explicit binary made %d cargo calls", n)
	}
}

func TestNoCargoModeValueMustBeOne(t *testing.T) {
	_, calls := prebuiltSetup(t)
	for _, value := range []string{"true", "yes", "0", "2"} {
		t.Setenv("DEVCOUNCIL_TEST_NO_CARGO", value)
		if msg := refused(t, "mode value "+value, DCStore); !strings.Contains(msg, "DEVCOUNCIL_TEST_NO_CARGO") {
			t.Errorf("value %q: the refusal does not name the variable:\n%s", value, msg)
		}
	}
	if n := cargoCalls(t, calls); n != 0 {
		t.Errorf("a misspelt mode made %d cargo calls", n)
	}
}

// devmap: DEVMAP_BIN keeps its old meaning without the mode, and is judged like
// the other binaries with it.
func TestDevmapFollowsTheMode(t *testing.T) {
	workspace, calls := prebuiltSetup(t)
	built := plantSource(t, workspace, "devmap-cli/src/main.rs", 2*time.Hour)
	explicit := filepath.Join(t.TempDir(), "devmap")
	plantBinary(t, explicit, time.Hour) // no dep-info, as a hand-installed devmap has none
	t.Setenv("DEVMAP_BIN", explicit)

	if got := Devmap(&recordingTB{}); got != explicit {
		t.Fatalf("without the mode DEVMAP_BIN must be returned as named, got %s", got)
	}

	t.Setenv("DEVCOUNCIL_TEST_NO_CARGO", "1")
	refused(t, "DEVMAP_BIN with no dep-info in the mode", Devmap)

	plantDepInfo(t, explicit, built)
	var got string
	if rec := probe(t, func(tb testing.TB) { got = Devmap(tb) }); !rec.returned || got == explicit {
		t.Fatalf("a checked DEVMAP_BIN should be handed out as a stable copy: got %q fatals %q", got, rec.fatals)
	}
	chtimes(t, built, time.Now())
	t.Setenv("DEVMAP_BIN", filepath.Join(t.TempDir(), "absent"))
	refused(t, "an absent DEVMAP_BIN in the mode", Devmap)

	t.Setenv("DEVMAP_BIN", "")
	plantBinary(t, debugBin(workspace, "devmap"), time.Hour, built)
	chtimes(t, built, time.Now())
	refused(t, "a stale target/debug/devmap in the mode", Devmap)
	if n := cargoCalls(t, calls); n != 0 {
		t.Errorf("devmap made %d cargo calls", n)
	}
}

func TestFreshnessFollowsSymlinkedCrates(t *testing.T) {
	workspace, _ := prebuiltSetup(t)
	// Manvi's layout: crates/<name> is a symlink to a crate elsewhere, and
	// dep-info names sources through the link.
	linked := filepath.Join(t.TempDir(), "dc-linked")
	src := plantSource(t, linked, "src/lib.rs", time.Second)
	if err := os.Symlink(linked, filepath.Join(workspace, "dc-linked")); err != nil {
		t.Fatal(err)
	}
	plantBinary(t, debugBin(workspace, "dcgrep"), time.Hour, filepath.Join(workspace, "dc-linked", "src", "lib.rs"))
	t.Setenv("DEVCOUNCIL_TEST_NO_CARGO", "1")
	if msg := refused(t, "a source newer than the binary behind a symlinked crate", DCGrep); !strings.Contains(msg, "lib.rs") {
		t.Errorf("the refusal does not name the newer source (%s):\n%s", src, msg)
	}
}

func TestParseDepInfo(t *testing.T) {
	got, err := parseDepInfo("/t/debug/dcstore: /a/one.rs /a/two\\ words.rs /a/three.sql\n/a/one.rs:\n/a/two\\ words.rs:\n")
	if err != nil {
		t.Fatal(err)
	}
	want := []string{"/a/one.rs", "/a/two words.rs", "/a/three.sql"}
	if strings.Join(got, "|") != strings.Join(want, "|") {
		t.Errorf("got %q, want %q", got, want)
	}
	for _, bad := range []string{"", "no separator", "/t/dcstore: \n"} {
		if _, err := parseDepInfo(bad); err == nil {
			t.Errorf("parseDepInfo(%q) accepted it", bad)
		}
	}
}
