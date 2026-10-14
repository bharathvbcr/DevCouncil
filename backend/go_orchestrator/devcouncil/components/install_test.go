package components

import (
	"encoding/json"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/version"
)

type fakeRunner struct {
	calls []string
}

func (f *fakeRunner) Run(name string, args []string, dir string, env []string) error {
	f.calls = append(f.calls, name+" "+strings.Join(args, " "))
	return nil
}

func TestDryRunDevmapDoesNotInvokeRunner(t *testing.T) {
	cs, err := Resolve([]string{"devmap"})
	if err != nil {
		t.Fatal(err)
	}
	r := &fakeRunner{}
	if err := Install(cs, Options{DryRun: true, Prefix: t.TempDir(), Runner: r, SourceRoot: ""}); err != nil {
		t.Fatal(err)
	}
	if len(r.calls) != 0 {
		t.Fatalf("dry-run invoked runner: %v", r.calls)
	}
}

func TestInstallDevmapFromCheckoutBuildsChecksAndInstalls(t *testing.T) {
	src := t.TempDir()
	if err := osMkdirCheckout(src); err != nil {
		t.Fatal(err)
	}
	t.Setenv("CARGO_TARGET_DIR", "")
	cs, err := Resolve([]string{"devmap"})
	if err != nil {
		t.Fatal(err)
	}
	r := &cargoRunner{t: t, src: src}
	prefix := t.TempDir()
	if err := Install(cs, Options{Prefix: prefix, Runner: r, Prober: healthyProber{}, SourceRoot: src, LookPath: noPath}); err != nil {
		t.Fatal(err)
	}
	if len(r.calls) != 1 || r.calls[0] != "cargo build --release -p devmap-cli --bin devmap" {
		t.Fatalf("calls=%v", r.calls)
	}
	if r.dirs[0] != filepath.Join(src, "rust") {
		t.Fatalf("cargo ran in %s", r.dirs[0])
	}
	got, err := os.ReadFile(filepath.Join(prefix, "bin", rustBinaryName("devmap")))
	if err != nil || string(got) != "built devmap" {
		t.Fatalf("installed binary = %q, %v", got, err)
	}
	st, _ := LoadState(prefix)
	if _, ok := st.Installed["devmap"]; !ok {
		t.Fatalf("receipt missing: %+v", st)
	}
	if _, ok := st.Installed["host"]; ok {
		t.Fatal("standalone devmap must not install the host")
	}
}

func TestUninstallOnlyRemovesReceiptBinary(t *testing.T) {
	prefix := t.TempDir()
	bindir := filepath.Join(prefix, "bin")
	if err := os.MkdirAll(bindir, 0o755); err != nil {
		t.Fatal(err)
	}
	ours := filepath.Join(bindir, "devmap")
	if err := os.WriteFile(ours, []byte("ours"), 0o755); err != nil {
		t.Fatal(err)
	}
	st, _ := LoadState(prefix)
	st.Record("devmap", ours, Origin{})
	if err := SaveState(st); err != nil {
		t.Fatal(err)
	}
	cs, err := Resolve([]string{"devmap"})
	if err != nil {
		t.Fatal(err)
	}
	if err := Uninstall(cs, Options{Prefix: prefix, Yes: true}); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(ours); !os.IsNotExist(err) {
		t.Fatalf("binary still present: %v", err)
	}
}

func TestDisablePersists(t *testing.T) {
	prefix := t.TempDir()
	if err := Disable("devmap", Options{Prefix: prefix}); err != nil {
		t.Fatal(err)
	}
	st, _ := LoadState(prefix)
	if !st.IsDisabled("devmap") {
		t.Fatal("expected disabled")
	}
	if err := Enable("devmap", Options{Prefix: prefix}); err != nil {
		t.Fatal(err)
	}
	if st, _ := LoadState(prefix); st.IsDisabled("devmap") {
		t.Fatal("still disabled")
	}
}

func osMkdirCheckout(root string) error {
	if err := os.MkdirAll(filepath.Join(root, "rust", "devmap-cli"), 0o755); err != nil {
		return err
	}
	if err := os.WriteFile(filepath.Join(root, "rust", "devmap-cli", "Cargo.toml"), []byte("[package]\nname=\"devmap-cli\"\n"), 0o644); err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Join(root, "backend", "go_orchestrator"), 0o755); err != nil {
		return err
	}
	if err := os.WriteFile(filepath.Join(root, "backend", "go_orchestrator", "go.mod"), []byte("module x\n"), 0o644); err != nil {
		return err
	}
	return nil
}

func TestPlannedCommandsStandaloneHasNoGoBuild(t *testing.T) {
	src := t.TempDir()
	if err := osMkdirCheckout(src); err != nil {
		t.Fatal(err)
	}
	cs, _ := Resolve([]string{"devmap"})
	cmds := PlannedCommands(cs, Options{SourceRoot: src, Prefix: "/opt"})
	joined := strings.Join(cmds, "\n")
	// By command word: "cargo build" contains the substring "go build".
	plansGo := func(cmds []string) bool {
		for _, cmd := range cmds {
			if strings.HasPrefix(cmd, "go ") || strings.Contains(cmd, "&& go ") || strings.Contains(cmd, "GOBIN=") {
				return true
			}
		}
		return false
	}
	if plansGo(cmds) {
		t.Fatalf("standalone planned a go command:\n%s", joined)
	}
	host, _ := Resolve([]string{"host"})
	if !plansGo(PlannedCommands(host, Options{SourceRoot: src, Prefix: "/opt"})) {
		t.Fatal("the go-command check does not recognise the host's own build")
	}
	if !strings.Contains(joined, "cargo build --release -p devmap-cli --bin devmap") || !strings.Contains(joined, "health-check") {
		t.Fatalf("%s", joined)
	}
}

// noCheckoutEnv puts the process where FindSourceRoot finds nothing.
func noCheckoutEnv(t *testing.T) {
	t.Helper()
	t.Setenv("DEVCOUNCIL_ROOT", "")
	t.Setenv("GITPULSE_DEVCOUNCIL_ROOT", "")
	t.Chdir(t.TempDir())
}

func TestInstallHostWithoutCheckoutRefusesAndNamesTheCheckoutRoute(t *testing.T) {
	for _, names := range [][]string{{"host"}, {}, {"all"}, {"host", "--dry-run"}} {
		noCheckoutEnv(t)
		r := &fakeRunner{}
		errOut, restoreErr := swapStderr(t)
		code := runInstall(append(append([]string{}, names...), "--prefix", t.TempDir()), Options{Runner: r})
		restoreErr()
		msg := errOut.String()
		if code != 2 {
			t.Errorf("%v: exit %d, want 2\n%s", names, code, msg)
		}
		for _, want := range []string{"git clone " + PublicGit, "devcouncil install --src <dir>"} {
			if !strings.Contains(msg, want) {
				t.Errorf("%v: refusal lacks %q:\n%s", names, want, msg)
			}
		}
		if len(r.calls) != 0 {
			t.Errorf("%v: refused install still ran %v", names, r.calls)
		}
	}
}

func TestPlannedCommandsHostWithoutCheckoutNeverPlansGoInstall(t *testing.T) {
	noCheckoutEnv(t)
	host, _ := Resolve([]string{"host"})
	for _, c := range PlannedCommands(host, Options{Prefix: "/opt"}) {
		if strings.Contains(c, "go install") || strings.Contains(c, "GOBIN=") {
			t.Fatalf("plan still contains the go install path: %s", c)
		}
	}
}

func TestInstallSrcFlagSelectsTheCheckout(t *testing.T) {
	noCheckoutEnv(t)
	real := t.TempDir()
	if err := osMkdirCheckout(real); err != nil {
		t.Fatal(err)
	}
	link := filepath.Join(t.TempDir(), "link")
	if err := os.Symlink(real, link); err != nil {
		t.Fatal(err)
	}
	rel, err := filepath.Rel(mustGetwd(t), real)
	if err != nil {
		t.Fatal(err)
	}
	for name, src := range map[string]string{"absolute": real, "symlink": link, "relative": rel} {
		out, restoreOut := swapStdout(t)
		code := runInstall([]string{"host", "--dry-run", "--src", src, "--prefix", "/opt"}, Options{})
		restoreOut()
		if code != 0 {
			t.Errorf("%s: exit %d", name, code)
		}
		if !strings.Contains(out.String(), "go -C ") || !strings.Contains(out.String(), "backend/go_orchestrator") {
			t.Errorf("%s: plan does not build from the checkout:\n%s", name, out.String())
		}
	}
}

func TestInstallSrcFlagRejectsWhatIsNotACheckout(t *testing.T) {
	noCheckoutEnv(t)
	file := filepath.Join(t.TempDir(), "f")
	if err := os.WriteFile(file, nil, 0o644); err != nil {
		t.Fatal(err)
	}
	for name, args := range map[string][]string{
		"missing":   {"--src", filepath.Join(t.TempDir(), "nope")},
		"empty dir": {"--src", t.TempDir()},
		"a file":    {"--src", file},
		"no value":  {"--src"},
		"empty":     {"--src="},
	} {
		r := &fakeRunner{}
		errOut, restoreErr := swapStderr(t)
		code := runInstall(append([]string{"devmap", "--prefix", t.TempDir()}, args...), Options{Runner: r})
		restoreErr()
		if code != 2 {
			t.Errorf("%s: exit %d, want 2\n%s", name, code, errOut.String())
		}
		if len(r.calls) != 0 {
			t.Errorf("%s: ran %v", name, r.calls)
		}
	}
}

// A remote install fetches from the public repository. Unpinned, two installs
// a day apart build different trees — possibly at a newer store schema than
// the host and its readers — so it pins the host's own release tag.
func TestRemoteInstallPinsTheHostsReleaseTag(t *testing.T) {
	noCheckoutEnv(t)
	want := "--tag v" + version.Version
	cs, err := Resolve([]string{"analysis"})
	if err != nil {
		t.Fatal(err)
	}
	for _, c := range PlannedCommands(cs, Options{Prefix: "/opt"}) {
		if !strings.Contains(c, want) {
			t.Errorf("planned remote install is unpinned: %s", c)
		}
	}
	r := &fakeRunner{}
	prefix := t.TempDir()
	if err := Install(cs, Options{Prefix: prefix, Runner: r, LookPath: noPath}); err != nil {
		t.Fatal(err)
	}
	if len(r.calls) != len(RustIDs(cs)) {
		t.Fatalf("calls = %v", r.calls)
	}
	for _, call := range r.calls {
		if !strings.Contains(call, want) {
			t.Errorf("ran an unpinned install: %s", call)
		}
	}
}

// The receipt names where each binary came from, and --list prints it.
func TestRemoteInstallRecordsItsRefAndListPrintsIt(t *testing.T) {
	noCheckoutEnv(t)
	prefix := t.TempDir()
	cs, _ := Resolve([]string{"devmap"})
	if err := Install(cs, Options{Prefix: prefix, Runner: &fakeRunner{}, LookPath: noPath}); err != nil {
		t.Fatal(err)
	}
	st, err := LoadState(prefix)
	if err != nil {
		t.Fatal(err)
	}
	wantRef := "tag v" + version.Version
	if got := st.Installed["devmap"].Origin; got != (Origin{Source: PublicGit, Ref: wantRef}) {
		t.Fatalf("receipt origin = %+v, want %s at %s", got, PublicGit, wantRef)
	}

	out, restore := swapStdout(t)
	code := runInstall([]string{"--list", "--prefix", prefix}, Options{})
	restore()
	if code != 0 || !strings.Contains(out.String(), "devmap") || !strings.Contains(out.String(), PublicGit+" at "+wantRef) {
		t.Fatalf("exit %d, --list does not show the ref:\n%s", code, out.String())
	}

	out, restore = swapStdout(t)
	code = runInstall([]string{"--list", "--json", "--prefix", prefix}, Options{})
	restore()
	var listed struct {
		Installed map[string]InstallRec `json:"installed"`
	}
	if code != 0 {
		t.Fatalf("exit %d", code)
	}
	if err := json.Unmarshal(out.Bytes(), &listed); err != nil {
		t.Fatalf("%v:\n%s", err, out.String())
	}
	if listed.Installed["devmap"].Ref != wantRef {
		t.Fatalf("--list --json installed = %+v", listed.Installed)
	}
}

func TestCheckoutInstallRecordsTheCheckout(t *testing.T) {
	src := t.TempDir()
	if err := osMkdirCheckout(src); err != nil {
		t.Fatal(err)
	}
	t.Setenv("CARGO_TARGET_DIR", "")
	cs, _ := Resolve([]string{"devmap"})
	prefix := t.TempDir()
	r := &cargoRunner{t: t, src: src}
	if err := Install(cs, Options{Prefix: prefix, Runner: r, Prober: healthyProber{}, SourceRoot: src, LookPath: noPath}); err != nil {
		t.Fatal(err)
	}
	st, _ := LoadState(prefix)
	if got := st.Installed["devmap"].Origin; got != (Origin{Source: src}) {
		t.Fatalf("receipt origin = %+v, want the checkout %s", got, src)
	}
}

// A host whose version is not a release has no tag to pin to. It refuses a
// remote install — with nothing run — and names the checkout route and --rev.
func TestDevBuildRefusesAnUnpinnedRemoteInstall(t *testing.T) {
	noCheckoutEnv(t)
	for _, v := range []string{"0.2.5-dev", "dev", "0.3", ""} {
		hostVersion := v
		if v == "" {
			hostVersion = "v0.2.4" // a tag spelled as a version is not a version
		}
		r := &fakeRunner{}
		prefix := t.TempDir()
		for _, args := range [][]string{{"devmap"}, {"analysis", "--dry-run"}} {
			errOut, restore := swapStderr(t)
			code := runInstall(append(args, "--prefix", prefix), Options{Runner: r, HostVersion: hostVersion})
			restore()
			if code != 2 {
				t.Errorf("%s %v: exit %d, want 2", hostVersion, args, code)
			}
			for _, want := range []string{"development build", "git clone " + PublicGit, "devcouncil install --src <dir>", "--rev"} {
				if !strings.Contains(errOut.String(), want) {
					t.Errorf("%s %v: refusal lacks %q:\n%s", hostVersion, args, want, errOut.String())
				}
			}
		}
		cs, _ := Resolve([]string{"devmap"})
		if err := Install(cs, Options{Prefix: prefix, Runner: r, HostVersion: hostVersion, LookPath: noPath}); err == nil {
			t.Errorf("%s: Install fetched without a pin", hostVersion)
		}
		if len(r.calls) != 0 {
			t.Errorf("%s: refused install ran %v", hostVersion, r.calls)
		}
		if st, _ := LoadState(prefix); len(st.Installed) != 0 {
			t.Errorf("%s: refused install wrote a receipt: %+v", hostVersion, st.Installed)
		}
	}
}

func TestRevPinsARemoteInstall(t *testing.T) {
	noCheckoutEnv(t)
	prefix := t.TempDir()
	r := &fakeRunner{}
	code := runInstall([]string{"devmap", "--rev", "0123abc", "--prefix", prefix}, Options{Runner: r, HostVersion: "0.2.5-dev", LookPath: noPath})
	if code != 0 {
		t.Fatalf("exit %d", code)
	}
	if len(r.calls) != 1 || !strings.Contains(r.calls[0], "--rev 0123abc") || strings.Contains(r.calls[0], "--tag") {
		t.Fatalf("calls = %v", r.calls)
	}
	st, _ := LoadState(prefix)
	if got := st.Installed["devmap"].Ref; got != "rev 0123abc" {
		t.Fatalf("receipt ref = %q", got)
	}

	for name, args := range map[string][]string{
		"flag-shaped": {"--rev=-x"},
		"space":       {"--rev", "a b"},
		"empty":       {"--rev="},
		"no value":    {"--rev"},
	} {
		r := &fakeRunner{}
		_, restore := swapStderr(t)
		code := runInstall(append([]string{"devmap", "--prefix", t.TempDir()}, args...), Options{Runner: r})
		restore()
		if code != 2 || len(r.calls) != 0 {
			t.Errorf("%s: exit %d, calls %v", name, code, r.calls)
		}
	}

	src := t.TempDir()
	if err := osMkdirCheckout(src); err != nil {
		t.Fatal(err)
	}
	r = &fakeRunner{}
	errOut, restore := swapStderr(t)
	code = runInstall([]string{"devmap", "--rev", "0123abc", "--src", src, "--prefix", t.TempDir()}, Options{Runner: r})
	restore()
	if code != 2 || len(r.calls) != 0 || !strings.Contains(errOut.String(), "--rev pins a remote install") {
		t.Fatalf("--rev with a checkout: exit %d, calls %v:\n%s", code, r.calls, errOut.String())
	}
}

// One removal failing must not leave the receipt claiming the components that
// were removed, nor hide the other failures: uninstall carries on, forgets
// what it deleted, saves, and names every component it could not remove.
func TestUninstallSavesWhatItRemovedAndNamesEveryFailure(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("a read-only directory does not stop deletion on Windows")
	}
	if os.Geteuid() == 0 {
		t.Skip("root deletes from a read-only directory")
	}
	prefix := t.TempDir()
	bindir := filepath.Join(prefix, "bin")
	locked := filepath.Join(prefix, "locked")
	for _, d := range []string{bindir, locked} {
		if err := os.MkdirAll(d, 0o755); err != nil {
			t.Fatal(err)
		}
	}
	host := filepath.Join(locked, "devcouncil")
	dcstore := filepath.Join(locked, "dcstore")
	devmap := filepath.Join(bindir, "devmap")
	for _, p := range []string{host, dcstore, devmap} {
		if err := os.WriteFile(p, []byte("bin"), 0o755); err != nil {
			t.Fatal(err)
		}
	}
	st, _ := LoadState(prefix)
	st.Record("host", host, Origin{})
	st.Record("devmap", devmap, Origin{})
	st.Record("dcstore", dcstore, Origin{})
	if err := SaveState(st); err != nil {
		t.Fatal(err)
	}
	if err := os.Chmod(locked, 0o555); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.Chmod(locked, 0o755) })

	cs, err := Resolve([]string{"host", "devmap", "dcstore"})
	if err != nil {
		t.Fatal(err)
	}
	err = Uninstall(cs, Options{Prefix: prefix, Yes: true})
	if err == nil {
		t.Fatal("uninstall reported success with two binaries still on disk")
	}
	for _, want := range []string{host, dcstore} {
		if !strings.Contains(err.Error(), want) {
			t.Errorf("error does not name %s: %v", want, err)
		}
	}
	if _, statErr := os.Stat(devmap); !os.IsNotExist(statErr) {
		t.Errorf("devmap was not removed after the host failed: %v", statErr)
	}
	got, loadErr := LoadState(prefix)
	if loadErr != nil {
		t.Fatal(loadErr)
	}
	if _, ok := got.Installed["devmap"]; ok {
		t.Errorf("receipt still lists the removed devmap: %+v", got.Installed)
	}
	for _, id := range []string{"host", "dcstore"} {
		if _, ok := got.Installed[id]; !ok {
			t.Errorf("receipt dropped %s, which is still on disk: %+v", id, got.Installed)
		}
	}

	// The CLI exits non-zero and prints every failure.
	errOut, restore := swapStderr(t)
	code := RunUninstall([]string{"host", "dcstore", "--yes", "--prefix", prefix})
	restore()
	if code != 1 {
		t.Errorf("exit %d, want 1", code)
	}
	for _, want := range []string{host, dcstore} {
		if !strings.Contains(errOut.String(), want) {
			t.Errorf("stderr does not name %s:\n%s", want, errOut.String())
		}
	}
}

func mustGetwd(t *testing.T) string {
	t.Helper()
	wd, err := os.Getwd()
	if err != nil {
		t.Fatal(err)
	}
	return wd
}
