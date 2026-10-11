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
