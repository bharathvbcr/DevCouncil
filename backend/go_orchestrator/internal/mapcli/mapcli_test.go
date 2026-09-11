package mapcli

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// The contract under test throughout this file: a --json invocation puts
// exactly one JSON object on stdout and every diagnostic on stderr.
//
// The Python original did not hold it — `dev campaign run --json` printed a
// banner to stdout ahead of its payload — and the tests there could not catch
// it, because Click's CliRunner merges the two streams into `result.output`.
// These tests read the streams separately, which is the only way the assertion
// means anything.

func runArgs(t *testing.T, env *Env, args ...string) (stdout, stderr string, code int) {
	t.Helper()
	var out, errBuf bytes.Buffer
	env.Stderr = &errBuf
	code = Run(context.Background(), env, &out, args)
	return out.String(), errBuf.String(), code
}

func TestJSONStdoutHoldsExactlyOneObjectOnSuccess(t *testing.T) {
	env := &Env{JSON: true, Root: t.TempDir(), Binary: fakeKernel(t, kernelStatusOK)}
	stdout, _, code := runArgs(t, env, "status")
	if code != 0 {
		t.Fatalf("exit code = %d, want 0 (stdout=%q)", code, stdout)
	}
	var payload map[string]any
	if err := json.Unmarshal([]byte(stdout), &payload); err != nil {
		t.Fatalf("stdout is not one JSON object: %v\nstdout=%q", err, stdout)
	}
	if payload["node_count"] != float64(1234) {
		t.Errorf("node_count = %v, want 1234", payload["node_count"])
	}
	if payload["binary"] == "" {
		t.Error("binary should be reported so a stale kernel is identifiable")
	}
}

func TestJSONStdoutHoldsExactlyOneObjectOnFailure(t *testing.T) {
	// A failing --json invocation still owes its caller one object. Emitting
	// nothing leaves an agent unable to tell a refusal from a crash.
	env := &Env{JSON: true, Root: t.TempDir(), Binary: fakeKernel(t, kernelStatusBroken)}
	stdout, stderr, code := runArgs(t, env, "status")
	if code == 0 {
		t.Fatal("a failing status must not exit 0")
	}
	var payload map[string]any
	if err := json.Unmarshal([]byte(stdout), &payload); err != nil {
		t.Fatalf("stdout is not one JSON object: %v\nstdout=%q", err, stdout)
	}
	if payload["ok"] != false {
		t.Errorf("ok = %v, want false", payload["ok"])
	}
	if stderr == "" {
		t.Error("the human-readable reason belongs on stderr")
	}
}

func TestUnknownSubcommandKeepsUsageOffStdout(t *testing.T) {
	// Usage text is a diagnostic. Under --json it must not reach the stream a
	// caller is parsing — this is precisely the shape of the Python bug.
	env := &Env{JSON: true, Root: t.TempDir()}
	stdout, stderr, code := runArgs(t, env, "nosuchthing")
	if code == 0 {
		t.Fatal("unknown subcommand must not exit 0")
	}
	if err := json.Unmarshal([]byte(stdout), &map[string]any{}); err != nil {
		t.Fatalf("stdout is not one JSON object: %v\nstdout=%q", err, stdout)
	}
	if !strings.Contains(stderr, "Usage") {
		t.Errorf("usage should be on stderr, got %q", stderr)
	}
	if strings.Contains(stdout, "Usage") {
		t.Errorf("usage leaked to stdout: %q", stdout)
	}
}

func TestHelpWritesNothingToStdout(t *testing.T) {
	// `dcmap --help` under --json has no payload to emit; prose on stdout
	// would be unparseable output from a successful invocation.
	env := &Env{JSON: true, Root: t.TempDir()}
	stdout, stderr, code := runArgs(t, env, "help")
	if code != 0 {
		t.Errorf("help exit = %d, want 0", code)
	}
	if stdout != "" {
		t.Errorf("stdout should be empty for help, got %q", stdout)
	}
	if !strings.Contains(stderr, "dcmap") {
		t.Errorf("help text should be on stderr, got %q", stderr)
	}
}

func TestHumanModeRendersATableNotJSON(t *testing.T) {
	env := &Env{JSON: false, Root: t.TempDir(), Binary: fakeKernel(t, kernelStatusOK)}
	stdout, _, code := runArgs(t, env, "status")
	if code != 0 {
		t.Fatalf("exit = %d", code)
	}
	if strings.HasPrefix(strings.TrimSpace(stdout), "{") {
		t.Errorf("human mode should not emit JSON: %q", stdout)
	}
	for _, want := range []string{"generation", "symbols", "kernel-view"} {
		if !strings.Contains(stdout, want) {
			t.Errorf("human output missing %q: %q", want, stdout)
		}
	}
}

func TestStatusRejectsStrayArguments(t *testing.T) {
	env := &Env{JSON: true, Root: t.TempDir(), Binary: fakeKernel(t, kernelStatusOK)}
	stdout, _, code := runArgs(t, env, "status", "extra")
	if code == 0 {
		t.Fatal("stray arguments must not be silently ignored")
	}
	var payload map[string]any
	if err := json.Unmarshal([]byte(stdout), &payload); err != nil {
		t.Fatalf("stdout is not one JSON object: %v", err)
	}
}

func TestDoctorReportsEveryCheckNotJustTheFirstFailure(t *testing.T) {
	// A doctor that stops at the first failure makes the operator re-run to
	// discover the next one. With no kernel there is genuinely nothing further
	// to measure, so it reports that and says so — rather than inventing
	// verdicts for checks it could not run.
	env := &Env{JSON: true, Root: t.TempDir()}
	t.Setenv("DEVMAP_BINARY", "")
	t.Setenv("PATH", t.TempDir()) // no devmap anywhere
	stdout, _, code := runArgs(t, env, "doctor")
	if code != 0 {
		t.Fatalf("doctor reports, it does not fail: exit = %d", code)
	}
	var payload doctorPayload
	if err := json.Unmarshal([]byte(stdout), &payload); err != nil {
		t.Fatalf("stdout is not one JSON object: %v\n%q", err, stdout)
	}
	if payload.OK {
		t.Error("doctor should not be OK with no kernel")
	}
	if payload.Remedy == "" {
		t.Error("a failing doctor must name a remedy")
	}
	if len(payload.Checks) == 0 {
		t.Error("doctor must report the checks it ran")
	}
}

func TestDelegatedCommandsAreNotReimplemented(t *testing.T) {
	// These eleven exist natively in the kernel. Wrapping them in Go would be a
	// forward-only layer over a CLI that already answers correctly, so the
	// registry marks them delegated and passes them through.
	for _, name := range delegatedNames {
		cmd, ok := lookup(name)
		if !ok {
			t.Errorf("%s missing from the registry", name)
			continue
		}
		if !cmd.Delegated {
			t.Errorf("%s should be delegated to the kernel, not ported", name)
		}
	}
}

func TestParseGlobalsStopsAtTheSubcommand(t *testing.T) {
	// Flags after the subcommand belong to the subcommand. That is what lets a
	// delegated command receive kernel flags this CLI has never heard of.
	env := &Env{Root: "."}
	rest, err := ParseGlobals(env, []string{"--json", "search", "--budget", "50", "foo"})
	if err != nil {
		t.Fatal(err)
	}
	if !env.JSON {
		t.Error("--json not parsed")
	}
	want := []string{"search", "--budget", "50", "foo"}
	if strings.Join(rest, " ") != strings.Join(want, " ") {
		t.Errorf("rest = %v, want %v", rest, want)
	}
}

func TestParseGlobalsRejectsRootWithoutValue(t *testing.T) {
	env := &Env{}
	if _, err := ParseGlobals(env, []string{"--root"}); err == nil {
		t.Error("--root with no value must be an error, not a silent default")
	}
}

func TestParseGlobalsRejectsNonDirectoryRoot(t *testing.T) {
	file := filepath.Join(t.TempDir(), "afile")
	if err := os.WriteFile(file, []byte("x"), 0o600); err != nil {
		t.Fatal(err)
	}
	env := &Env{}
	if _, err := ParseGlobals(env, []string{"--root", file}); err == nil {
		t.Error("a file as --root must be rejected")
	}
}

func TestParseGlobalsDoubleDashEndsGlobalFlags(t *testing.T) {
	env := &Env{Root: "."}
	rest, err := ParseGlobals(env, []string{"--json", "--", "--json"})
	if err != nil {
		t.Fatal(err)
	}
	// The second --json is past the separator, so it is the subcommand name,
	// however odd — not a second global flag.
	if len(rest) != 1 || rest[0] != "--json" {
		t.Errorf("rest = %v, want [--json]", rest)
	}
}

// An explicit override wins over a local build and PATH alike when it answers
// the probe; the two refusals are tested further down. Asserted through
// discoverBinary, not binaryCandidates: the override is a decision taken
// before the candidate list exists.
func TestDiscoveryPrefersAnExplicitBinaryOverLocalBuildsAndPath(t *testing.T) {
	explicit := fakeKernel(t, kernelStatusOK)
	onPath := fakeKernel(t, kernelStatusOK)
	root := t.TempDir()
	local := filepath.Join(root, "rust", "target", "release", "devmap")
	if err := os.MkdirAll(filepath.Dir(local), 0o755); err != nil {
		t.Fatal(err)
	}
	script, err := os.ReadFile(explicit)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(local, script, 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("DEVMAP_BINARY", explicit)
	t.Setenv("PATH", filepath.Dir(onPath))

	got, err := discoverBinary(context.Background(), root)
	if err != nil {
		t.Fatalf("a capable explicit override must be used: %v", err)
	}
	if got != explicit {
		t.Fatalf("discoverBinary = %s, want the explicit %s over local %s and PATH %s", got, explicit, local, onPath)
	}
}

func TestDiscoveryPrefersALocalBuildOverPath(t *testing.T) {
	// PATH resolves whatever was last `cargo install`ed, which is independent
	// of the working tree and routinely months stale. A build inside the
	// repository must win, or a kernel change is not exercised by the next
	// command that depends on it.
	root := t.TempDir()
	local := filepath.Join(root, "rust", "target", "release", "devmap")
	if err := os.MkdirAll(filepath.Dir(local), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(local, []byte("#!/bin/sh\nexit 0\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	onPath := t.TempDir()
	if err := os.WriteFile(filepath.Join(onPath, "devmap"), []byte("#!/bin/sh\nexit 0\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("DEVMAP_BINARY", "")
	t.Setenv("PATH", onPath)

	got := binaryCandidates(root)
	if len(got) == 0 {
		t.Fatal("no candidates found")
	}
	if got[0] != local {
		t.Errorf("candidates[0] = %s, want the local build %s", got[0], local)
	}
}

func TestDiscoveryFindsLaneTargetDirectories(t *testing.T) {
	// Concurrent fix lanes each build into their own CARGO_TARGET_DIR
	// (`rust/target-<lane>`), so that is where a current build usually is.
	root := t.TempDir()
	lane := filepath.Join(root, "rust", "target-lane1", "release", "devmap")
	if err := os.MkdirAll(filepath.Dir(lane), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(lane, []byte("#!/bin/sh\nexit 0\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("DEVMAP_BINARY", "")
	t.Setenv("PATH", t.TempDir())

	got := binaryCandidates(root)
	if len(got) != 1 || got[0] != lane {
		t.Errorf("candidates = %v, want [%s]", got, lane)
	}
}

func TestDiscoveryReportsNoKernelDistinctly(t *testing.T) {
	t.Setenv("DEVMAP_BINARY", "")
	t.Setenv("PATH", t.TempDir())
	_, err := discoverBinary(context.Background(), t.TempDir())
	if !errors.Is(err, ErrNoBinary) {
		t.Errorf("err = %v, want ErrNoBinary so callers can tell it from a broken map", err)
	}
}

// --- fake kernel -----------------------------------------------------------

const (
	kernelStatusOK = `{"db_path":"/tmp/devmap.sqlite","generation_id":7,"node_count":1234,` +
		`"edge_count":5678,"pending_count":0,"quarantined_count":0,"is_fresh":true,"degraded_reason":null}`
	kernelStatusEmpty = `{"db_path":"/tmp/devmap.sqlite","generation_id":0,"node_count":0,` +
		`"edge_count":0,"pending_count":0,"quarantined_count":0,"is_fresh":false,"degraded_reason":null}`
	kernelStatusDegraded = `{"db_path":"/tmp/devmap.sqlite","generation_id":7,"node_count":1234,` +
		`"edge_count":5678,"pending_count":0,"quarantined_count":0,"is_fresh":false,` +
		`"degraded_reason":"unlinked grammar vb"}`
	kernelStatusBroken = ""
)

// subcommandShim finds the subcommand in the child's argv.
//
// The client invokes `devmap --json status`, not `devmap status`: the kernel
// declares --json on the root command, so global flags precede the subcommand.
// A fake that read $1 would answer the probe and then silently return nothing
// for every real call — which is exactly how this shim came to exist.
const subcommandShim = `sub=""
for a in "$@"; do
  case "$a" in
    -*) ;;
    *) sub="$a"; break ;;
  esac
done`

// fakeKernel writes a shell script that answers the capability probe and then
// returns the given status JSON. It exists so these tests exercise the CLI's
// own logic without depending on a built Rust binary — a dependency that would
// make the suite pass or fail on what someone last installed.
func fakeKernel(t *testing.T, statusJSON string) string {
	t.Helper()
	dir := t.TempDir()
	path := filepath.Join(dir, "devmap")
	var body string
	if statusJSON == "" {
		// Answers the probe, then fails the real call: the shape of a kernel
		// that is present but cannot serve this repository.
		body = `#!/bin/sh
` + subcommandShim + `
case "$sub" in
  manifest) echo "--graph-output"; exit 0 ;;
  *) echo "store is corrupt" >&2; exit 1 ;;
esac
`
	} else {
		body = `#!/bin/sh
` + subcommandShim + `
case "$sub" in
  manifest) echo "--graph-output"; exit 0 ;;
  status) echo '` + statusJSON + `'; exit 0 ;;
  search) echo '{"ok":true,"hits":[]}'; exit 0 ;;
  *) echo '{"ok":true}'; exit 0 ;;
esac
`
	}
	if err := os.WriteFile(path, []byte(body), 0o755); err != nil {
		t.Fatal(err)
	}
	return path
}

// An operator naming a binary is entitled to have that binary used — or told
// why it cannot be. Measured with the built dcmap: DEVMAP_BINARY pointing at a
// script that printed garbage, hung, or exited 3 was silently replaced by
// ~/.cargo/bin/devmap, and `status` answered with that binary's name — the
// exact substitution binaryCandidates' own comment says must never happen.
func TestAnExplicitBinaryThatFailsTheProbeIsRefusedNotSubstituted(t *testing.T) {
	broken := filepath.Join(t.TempDir(), "devmap-broken")
	if err := os.WriteFile(broken, []byte("#!/bin/sh\necho kernel exploded >&2\nexit 3\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	capable := fakeKernel(t, kernelStatusOK)
	t.Setenv("DEVMAP_BINARY", broken)
	t.Setenv("PATH", filepath.Dir(capable))

	got, err := discoverBinary(context.Background(), t.TempDir())
	if err == nil {
		t.Fatalf("DEVMAP_BINARY=%s failed the probe and was silently replaced by %s", broken, got)
	}
	if !strings.Contains(err.Error(), broken) {
		t.Errorf("the refusal must name the override: %v", err)
	}
	if errors.Is(err, ErrNoBinary) {
		t.Errorf("an override that failed is not %q: %v", ErrNoBinary, err)
	}
}

func TestAnExplicitBinaryThatDoesNotExistIsRefusedByName(t *testing.T) {
	missing := filepath.Join(t.TempDir(), "no-such-devmap")
	capable := fakeKernel(t, kernelStatusOK)
	t.Setenv("DEVMAP_BINARY", missing)
	t.Setenv("PATH", filepath.Dir(capable))

	got, err := discoverBinary(context.Background(), t.TempDir())
	if err == nil {
		t.Fatalf("DEVMAP_BINARY=%s does not exist and was silently replaced by %s", missing, got)
	}
	if !strings.Contains(err.Error(), missing) {
		t.Errorf("the refusal must name the override: %v", err)
	}
}

func TestAnExplicitBinaryThatIsADirectoryIsRefusedByName(t *testing.T) {
	dir := t.TempDir()
	capable := fakeKernel(t, kernelStatusOK)
	t.Setenv("DEVMAP_BINARY", dir)
	t.Setenv("PATH", filepath.Dir(capable))

	got, err := discoverBinary(context.Background(), t.TempDir())
	if err == nil {
		t.Fatalf("DEVMAP_BINARY=%s is a directory and was silently replaced by %s", dir, got)
	}
	if !strings.Contains(err.Error(), dir) || !strings.Contains(err.Error(), "directory") {
		t.Errorf("the refusal must name the directory: %v", err)
	}
}

func TestDiscoveryFallsThroughAnIncapableLocalBuildToACapablePathBinary(t *testing.T) {
	root := t.TempDir()
	local := filepath.Join(root, "rust", "target", "release", "devmap")
	if err := os.MkdirAll(filepath.Dir(local), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(local, []byte("#!/bin/sh\nexit 1\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	capable := fakeKernel(t, kernelStatusOK)
	t.Setenv("DEVMAP_BINARY", "")
	t.Setenv("PATH", filepath.Dir(capable))

	got, err := discoverBinary(context.Background(), root)
	if err != nil {
		t.Fatalf("an incapable local build must fall through to PATH: %v", err)
	}
	if got != capable {
		t.Fatalf("discoverBinary = %s, want the capable PATH binary %s, not the local %s", got, capable, local)
	}
}

func TestParseGlobalsRootEqualsFormSetsAnAbsoluteDirectory(t *testing.T) {
	dir := t.TempDir()
	env := &Env{}
	rest, err := ParseGlobals(env, []string{"--root=" + dir, "status"})
	if err != nil {
		t.Fatal(err)
	}
	if len(rest) != 1 || rest[0] != "status" {
		t.Errorf("rest = %v, want [status]", rest)
	}
	if env.Root != dir && !strings.HasPrefix(env.Root, "/") {
		t.Errorf("root = %q, want an absolute path for %q", env.Root, dir)
	}
	info, err := os.Stat(env.Root)
	if err != nil || !info.IsDir() {
		t.Errorf("resolved root %q is not a directory: %v", env.Root, err)
	}
}

func TestParseGlobalsRejectsAnEmptyRootValue(t *testing.T) {
	env := &Env{}
	if _, err := ParseGlobals(env, []string{"--root="}); err == nil {
		t.Error("an empty --root= value must be an error")
	}
}

func TestDefaultRootIsAbsolute(t *testing.T) {
	got := DefaultRoot()
	if got == "" {
		t.Fatal("DefaultRoot returned empty")
	}
	if got != "." && !filepath.IsAbs(got) {
		t.Errorf("DefaultRoot = %q, want an absolute path", got)
	}
}

func TestDiagfOnNilStderrIsANoop(t *testing.T) {
	env := &Env{}
	env.Diagf("this must not panic")
}

func TestStatusDiscoversTheKernelWhenBinaryIsUnset(t *testing.T) {
	kernel := fakeKernel(t, kernelStatusOK)
	t.Setenv("DEVMAP_BINARY", kernel)
	t.Setenv("PATH", t.TempDir())
	env := &Env{JSON: true, Root: t.TempDir()}
	stdout, _, code := runArgs(t, env, "status")
	if code != 0 {
		t.Fatalf("exit = %d, stdout=%q", code, stdout)
	}
	var payload statusPayload
	if err := json.Unmarshal([]byte(stdout), &payload); err != nil {
		t.Fatalf("stdout is not a status payload: %v\n%q", err, stdout)
	}
	if payload.Binary != kernel {
		t.Errorf("binary = %q, want the discovered %s", payload.Binary, kernel)
	}
	if !payload.KernelFresh {
		t.Error("kernel-view should be fresh for this fixture")
	}
}

func TestStatusHumanModeReportsADegradedKernel(t *testing.T) {
	env := &Env{JSON: false, Root: t.TempDir(), Binary: fakeKernel(t, kernelStatusDegraded)}
	stdout, _, code := runArgs(t, env, "status")
	if code != 0 {
		t.Fatalf("exit = %d", code)
	}
	for _, want := range []string{"not fresh", "unlinked grammar"} {
		if !strings.Contains(stdout, want) {
			t.Errorf("human status missing %q: %q", want, stdout)
		}
	}
}

func TestDoctorReportsAUsableStore(t *testing.T) {
	kernel := fakeKernel(t, kernelStatusOK)
	t.Setenv("DEVMAP_BINARY", kernel)
	t.Setenv("PATH", t.TempDir())
	env := &Env{JSON: true, Root: t.TempDir()}
	stdout, _, code := runArgs(t, env, "doctor")
	if code != 0 {
		t.Fatalf("doctor reports, it does not fail: exit = %d stdout=%q", code, stdout)
	}
	var payload doctorPayload
	if err := json.Unmarshal([]byte(stdout), &payload); err != nil {
		t.Fatalf("stdout is not one JSON object: %v\n%q", err, stdout)
	}
	if !payload.OK {
		t.Fatalf("doctor should be OK with a usable store: %+v", payload)
	}
	if payload.Binary != kernel {
		t.Errorf("binary = %q, want %s", payload.Binary, kernel)
	}
	if payload.Remedy != "" {
		t.Errorf("a passing doctor must not prescribe a fix, got %q", payload.Remedy)
	}
	names := doctorCheckNames(payload)
	for _, want := range []string{"kernel", "store", "generation"} {
		if !names[want] {
			t.Errorf("missing check %q in %+v", want, payload.Checks)
		}
	}
}

func TestDoctorReportsAnUnreadableStoreWithoutInventingAGeneration(t *testing.T) {
	kernel := fakeKernel(t, kernelStatusBroken)
	t.Setenv("DEVMAP_BINARY", kernel)
	t.Setenv("PATH", t.TempDir())
	env := &Env{JSON: true, Root: t.TempDir()}
	stdout, _, code := runArgs(t, env, "doctor")
	if code != 0 {
		t.Fatalf("doctor reports, it does not fail: exit = %d stdout=%q", code, stdout)
	}
	var payload doctorPayload
	if err := json.Unmarshal([]byte(stdout), &payload); err != nil {
		t.Fatalf("stdout is not one JSON object: %v\n%q", err, stdout)
	}
	if payload.OK {
		t.Fatal("doctor should not be OK when status fails")
	}
	if payload.Remedy != "devmap build" {
		t.Errorf("remedy = %q, want devmap build", payload.Remedy)
	}
	names := doctorCheckNames(payload)
	if !names["store"] {
		t.Errorf("store check missing: %+v", payload.Checks)
	}
	if names["generation"] {
		t.Error("generation must not be invented when the store could not be read")
	}
}

func TestDoctorReportsAnEmptyGeneration(t *testing.T) {
	kernel := fakeKernel(t, kernelStatusEmpty)
	t.Setenv("DEVMAP_BINARY", kernel)
	t.Setenv("PATH", t.TempDir())
	env := &Env{JSON: true, Root: t.TempDir()}
	stdout, _, code := runArgs(t, env, "doctor")
	if code != 0 {
		t.Fatalf("exit = %d stdout=%q", code, stdout)
	}
	var payload doctorPayload
	if err := json.Unmarshal([]byte(stdout), &payload); err != nil {
		t.Fatalf("stdout is not one JSON object: %v\n%q", err, stdout)
	}
	if payload.OK {
		t.Fatal("an empty generation is not a usable map")
	}
	if payload.Remedy != "devmap build" {
		t.Errorf("remedy = %q, want devmap build", payload.Remedy)
	}
	found := false
	for _, c := range payload.Checks {
		if c.Name == "generation" {
			found = true
			if c.Passed {
				t.Error("generation check should fail")
			}
		}
	}
	if !found {
		t.Fatalf("generation check missing: %+v", payload.Checks)
	}
}

func TestDoctorReportsDegradationWithoutPrescribingARebuildLoop(t *testing.T) {
	kernel := fakeKernel(t, kernelStatusDegraded)
	t.Setenv("DEVMAP_BINARY", kernel)
	t.Setenv("PATH", t.TempDir())
	env := &Env{JSON: true, Root: t.TempDir()}
	stdout, _, code := runArgs(t, env, "doctor")
	if code != 0 {
		t.Fatalf("exit = %d stdout=%q", code, stdout)
	}
	var payload doctorPayload
	if err := json.Unmarshal([]byte(stdout), &payload); err != nil {
		t.Fatalf("stdout is not one JSON object: %v\n%q", err, stdout)
	}
	if payload.OK {
		t.Fatal("a degraded kernel is not OK")
	}
	if !strings.Contains(payload.Remedy, "unlinked grammars are permanent") {
		t.Errorf("remedy must not send the operator around a rebuild loop: %q", payload.Remedy)
	}
	found := false
	for _, c := range payload.Checks {
		if c.Name == "degraded" {
			found = true
			if c.Passed || !strings.Contains(c.Detail, "unlinked grammar") {
				t.Errorf("degraded check = %+v", c)
			}
		}
	}
	if !found {
		t.Fatalf("degraded check missing: %+v", payload.Checks)
	}
}

func TestDoctorRejectsStrayArguments(t *testing.T) {
	env := &Env{JSON: true, Root: t.TempDir(), Binary: fakeKernel(t, kernelStatusOK)}
	stdout, _, code := runArgs(t, env, "doctor", "extra")
	if code == 0 {
		t.Fatal("stray arguments must not be silently ignored")
	}
	var payload map[string]any
	if err := json.Unmarshal([]byte(stdout), &payload); err != nil {
		t.Fatalf("stdout is not one JSON object: %v", err)
	}
	if payload["ok"] != false {
		t.Errorf("ok = %v, want false", payload["ok"])
	}
}

func TestDoctorHumanModeRendersEveryCheck(t *testing.T) {
	kernel := fakeKernel(t, kernelStatusOK)
	t.Setenv("DEVMAP_BINARY", kernel)
	t.Setenv("PATH", t.TempDir())
	env := &Env{JSON: false, Root: t.TempDir()}
	stdout, _, code := runArgs(t, env, "doctor")
	if code != 0 {
		t.Fatalf("exit = %d stdout=%q", code, stdout)
	}
	if strings.HasPrefix(strings.TrimSpace(stdout), "{") {
		t.Errorf("human mode should not emit JSON: %q", stdout)
	}
	for _, want := range []string{"kernel", "store", "generation", "[ok"} {
		if !strings.Contains(stdout, want) {
			t.Errorf("human doctor missing %q: %q", want, stdout)
		}
	}
}

func TestDoctorHumanModePrintsAFixWhenTheKernelIsMissing(t *testing.T) {
	env := &Env{JSON: false, Root: t.TempDir()}
	t.Setenv("DEVMAP_BINARY", "")
	t.Setenv("PATH", t.TempDir())
	stdout, _, code := runArgs(t, env, "doctor")
	if code != 0 {
		t.Fatalf("exit = %d stdout=%q", code, stdout)
	}
	if !strings.Contains(stdout, "[FAIL]") {
		t.Errorf("human doctor should mark the failure: %q", stdout)
	}
	if !strings.Contains(stdout, "fix:") {
		t.Errorf("human doctor should print the remedy: %q", stdout)
	}
}

func TestDelegatedJSONFlagIsPlacedBeforeTheSubcommand(t *testing.T) {
	var captured bytes.Buffer
	passthroughStdout = &captured
	t.Cleanup(func() { passthroughStdout = os.Stdout })

	path := filepath.Join(t.TempDir(), "devmap")
	if err := os.WriteFile(path, []byte("#!/bin/sh\nprintf '%s\\n' \"$@\"\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	env := &Env{JSON: true, Root: t.TempDir(), Binary: path}
	stdout, stderr, code := runArgs(t, env, "search", "foo")
	if code != 0 {
		t.Fatalf("exit = %d stderr=%q", code, stderr)
	}
	if stdout != "" {
		t.Errorf("Run must not write beside the child, got %q", stdout)
	}
	got := strings.Split(strings.TrimSpace(captured.String()), "\n")
	want := []string{"--json", "search", "foo"}
	if strings.Join(got, " ") != strings.Join(want, " ") {
		t.Errorf("argv = %v, want %v", got, want)
	}
}

func TestDelegatedCommandStreamsKernelStdoutWithoutASecondObject(t *testing.T) {
	var captured bytes.Buffer
	passthroughStdout = &captured
	t.Cleanup(func() { passthroughStdout = os.Stdout })

	kernel := fakeKernel(t, kernelStatusOK)
	env := &Env{JSON: true, Root: t.TempDir(), Binary: kernel}
	stdout, stderr, code := runArgs(t, env, "search", "foo")
	if code != 0 {
		t.Fatalf("delegated search exit = %d stderr=%q", code, stderr)
	}
	if stdout != "" {
		t.Errorf("Run must not append a second JSON object beside the kernel's, got %q", stdout)
	}
	var payload map[string]any
	if err := json.Unmarshal(captured.Bytes(), &payload); err != nil {
		t.Fatalf("kernel stdout is not one JSON object: %v\n%q", err, captured.String())
	}
	if payload["ok"] != true {
		t.Errorf("kernel payload = %v", payload)
	}
}

func TestDelegatedCommandWithoutJSONDoesNotInjectTheFlag(t *testing.T) {
	var captured bytes.Buffer
	passthroughStdout = &captured
	t.Cleanup(func() { passthroughStdout = os.Stdout })

	kernel := fakeKernel(t, kernelStatusOK)
	env := &Env{JSON: false, Root: t.TempDir(), Binary: kernel}
	stdout, stderr, code := runArgs(t, env, "search", "foo")
	if code != 0 {
		t.Fatalf("exit = %d stderr=%q", code, stderr)
	}
	if stdout != "" {
		t.Errorf("delegated stdout belongs to the child, got %q on Run's writer", stdout)
	}
	if !strings.Contains(captured.String(), `"ok": true`) && !strings.Contains(captured.String(), `"ok":true`) {
		t.Errorf("kernel output missing: %q", captured.String())
	}
}

func TestDelegatedCommandReportsAKernelFailure(t *testing.T) {
	kernel := fakeKernel(t, kernelStatusBroken)
	env := &Env{JSON: true, Root: t.TempDir(), Binary: kernel}
	stdout, stderr, code := runArgs(t, env, "search", "foo")
	if code == 0 {
		t.Fatal("a failing kernel must not exit 0")
	}
	var payload map[string]any
	if err := json.Unmarshal([]byte(stdout), &payload); err != nil {
		t.Fatalf("stdout is not one JSON object: %v\n%q", err, stdout)
	}
	if payload["ok"] != false {
		t.Errorf("ok = %v, want false", payload["ok"])
	}
	if !strings.Contains(stderr, "devmap search") {
		t.Errorf("stderr should name the delegated command, got %q", stderr)
	}
}

func TestDelegatedCommandRefusesWhenNoKernelExists(t *testing.T) {
	t.Setenv("DEVMAP_BINARY", "")
	t.Setenv("PATH", t.TempDir())
	env := &Env{JSON: true, Root: t.TempDir()}
	stdout, _, code := runArgs(t, env, "search", "foo")
	if code == 0 {
		t.Fatal("search with no kernel must not exit 0")
	}
	var payload map[string]any
	if err := json.Unmarshal([]byte(stdout), &payload); err != nil {
		t.Fatalf("stdout is not one JSON object: %v", err)
	}
}

func doctorCheckNames(payload doctorPayload) map[string]bool {
	names := map[string]bool{}
	for _, c := range payload.Checks {
		names[c.Name] = true
	}
	return names
}
