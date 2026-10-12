package components

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
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
	st.Record("devmap", ours)
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

func mustGetwd(t *testing.T) string {
	t.Helper()
	wd, err := os.Getwd()
	if err != nil {
		t.Fatal(err)
	}
	return wd
}
