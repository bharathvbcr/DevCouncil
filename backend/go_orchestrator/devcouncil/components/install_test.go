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

func TestInstallDevmapFromCheckoutCallsComponentsScript(t *testing.T) {
	src := t.TempDir()
	if err := osMkdirCheckout(src); err != nil {
		t.Fatal(err)
	}
	cs, err := Resolve([]string{"devmap"})
	if err != nil {
		t.Fatal(err)
	}
	r := &fakeRunner{}
	prefix := t.TempDir()
	if err := Install(cs, Options{Prefix: prefix, Runner: r, SourceRoot: src}); err != nil {
		t.Fatal(err)
	}
	if len(r.calls) != 1 {
		t.Fatalf("calls=%v", r.calls)
	}
	if !strings.Contains(r.calls[0], "install-components.sh") || !strings.Contains(r.calls[0], "devmap") {
		t.Fatalf("call=%q", r.calls[0])
	}
	st := LoadState(prefix)
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
	st := LoadState(prefix)
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
	st := LoadState(prefix)
	if !st.IsDisabled("devmap") {
		t.Fatal("expected disabled")
	}
	if err := Enable("devmap", Options{Prefix: prefix}); err != nil {
		t.Fatal(err)
	}
	if LoadState(prefix).IsDisabled("devmap") {
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
	if err := os.MkdirAll(filepath.Join(root, "scripts"), 0o755); err != nil {
		return err
	}
	return os.WriteFile(filepath.Join(root, "scripts", "install-components.sh"), []byte("#!/bin/sh\n"), 0o755)
}

func TestPlannedCommandsStandaloneHasNoGoBuild(t *testing.T) {
	src := t.TempDir()
	if err := osMkdirCheckout(src); err != nil {
		t.Fatal(err)
	}
	cs, _ := Resolve([]string{"devmap"})
	cmds := PlannedCommands(cs, Options{SourceRoot: src, Prefix: "/opt"})
	joined := strings.Join(cmds, "\n")
	if strings.Contains(joined, "go ") {
		t.Fatalf("standalone planned a go command:\n%s", joined)
	}
	if !strings.Contains(joined, "install-components.sh") || !strings.Contains(joined, "devmap") {
		t.Fatalf("%s", joined)
	}
}
