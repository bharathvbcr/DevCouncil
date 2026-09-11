package components

import (
	"encoding/json"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"testing"
)

func TestDryRunUninstallDoesNotRemoveDevSymlink(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("unix symlink")
	}
	prefix := t.TempDir()
	bindir := filepath.Join(prefix, "bin")
	if err := os.MkdirAll(bindir, 0o755); err != nil {
		t.Fatal(err)
	}
	host := filepath.Join(bindir, "devcouncil")
	if err := os.WriteFile(host, []byte("host"), 0o755); err != nil {
		t.Fatal(err)
	}
	dev := filepath.Join(bindir, "dev")
	if err := os.Symlink("devcouncil", dev); err != nil {
		t.Fatal(err)
	}
	st := LoadState(prefix)
	st.Record("host", host)
	if err := SaveState(st); err != nil {
		t.Fatal(err)
	}
	cs, err := Resolve([]string{"host"})
	if err != nil {
		t.Fatal(err)
	}
	if err := Uninstall(cs, Options{Prefix: prefix, DryRun: true, Yes: true}); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Lstat(dev); err != nil {
		t.Fatalf("dry-run removed the dev symlink: %v", err)
	}
	if _, err := os.Stat(host); err != nil {
		t.Fatalf("dry-run removed the host binary: %v", err)
	}
}

func TestUninstallRefusesReceiptOutsidePrefix(t *testing.T) {
	prefix := t.TempDir()
	outside := filepath.Join(t.TempDir(), "precious")
	if err := os.WriteFile(outside, []byte("keep"), 0o644); err != nil {
		t.Fatal(err)
	}
	st := LoadState(prefix)
	st.Record("devmap", outside)
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
	if _, err := os.Stat(outside); err != nil {
		t.Fatalf("uninstalled a file outside prefix: %v", err)
	}
}

func TestUninstallRefusesDotDotReceipt(t *testing.T) {
	prefix := t.TempDir()
	bindir := filepath.Join(prefix, "bin")
	if err := os.MkdirAll(bindir, 0o755); err != nil {
		t.Fatal(err)
	}
	victim := filepath.Join(t.TempDir(), "victim")
	if err := os.WriteFile(victim, []byte("keep"), 0o644); err != nil {
		t.Fatal(err)
	}
	st := LoadState(prefix)
	st.Record("dcgrep", filepath.Join(bindir, "..", "..", filepath.Base(t.TempDir()), "nope"))
	// Point the receipt at victim via .. after a fake join that cleans to outside.
	st.Installed["dcgrep"] = InstallRec{Binary: victim, At: "now"}
	if err := SaveState(st); err != nil {
		t.Fatal(err)
	}
	cs, err := Resolve([]string{"dcgrep"})
	if err != nil {
		t.Fatal(err)
	}
	if err := Uninstall(cs, Options{Prefix: prefix, Yes: true}); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(victim); err != nil {
		t.Fatalf("dot-dot receipt deleted a foreign file: %v", err)
	}
}

func TestUninstallDoesNotRemoveDirectoryNamedLikeBinary(t *testing.T) {
	prefix := t.TempDir()
	bindir := filepath.Join(prefix, "bin")
	dir := filepath.Join(bindir, "devmap")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	marker := filepath.Join(dir, "keep")
	if err := os.WriteFile(marker, []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	st := LoadState(prefix)
	st.Record("devmap", dir)
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
	if _, err := os.Stat(marker); err != nil {
		t.Fatalf("removed a directory named like a binary: %v", err)
	}
}

func TestCorruptStateDoesNotKeepPartialMaps(t *testing.T) {
	prefix := t.TempDir()
	path := filepath.Join(prefix, "share", "devcouncil", "components.json")
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	// Valid prefix then truncated payload — Unmarshal must not keep a half map.
	if err := os.WriteFile(path, []byte(`{"version":1,"installed":{"devmap":{"binary":"/tmp/x"},"dc`), 0o644); err != nil {
		t.Fatal(err)
	}
	st := LoadState(prefix)
	if len(st.Installed) != 0 {
		t.Fatalf("corrupt JSON leaked installed=%v", st.Installed)
	}
	if len(st.Disabled) != 0 {
		t.Fatalf("corrupt JSON leaked disabled=%v", st.Disabled)
	}
}

func TestLoadStateMergesLegacyDisabledFile(t *testing.T) {
	prefix := t.TempDir()
	dir := filepath.Join(prefix, "share", "devcouncil")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "disabled"), []byte("devmap\nuv\ndevmap\n*\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	st := LoadState(prefix)
	if !st.IsDisabled("devmap") {
		t.Fatal("legacy disabled file was ignored")
	}
	if st.IsDisabled("uv") || st.IsDisabled("*") {
		t.Fatalf("unknown disable marks were kept: %v", st.Disabled)
	}
	if err := SaveState(st); err != nil {
		t.Fatal(err)
	}
	st2 := LoadState(prefix)
	if !st2.IsDisabled("devmap") {
		t.Fatal("merged disable did not round-trip")
	}
}

func TestRemoteCargoHonorsPrefixRoot(t *testing.T) {
	cs, err := Resolve([]string{"devmap"})
	if err != nil {
		t.Fatal(err)
	}
	prefix := filepath.Join(t.TempDir(), "with space")
	cmds := PlannedCommands(cs, Options{SourceRoot: t.TempDir(), Prefix: prefix})
	joined := strings.Join(cmds, "\n")
	if !strings.Contains(joined, "--root") {
		t.Fatalf("remote cargo install ignored PREFIX:\n%s", joined)
	}
	if !strings.Contains(joined, prefix) {
		t.Fatalf("remote cargo missing prefix %q:\n%s", prefix, joined)
	}
	if strings.Contains(joined, "install-components.sh") {
		t.Fatalf("bogus SourceRoot still planned a checkout script:\n%s", joined)
	}
}

func TestInstallRustUsesStructuredArgsNotFields(t *testing.T) {
	cs, err := Resolve([]string{"dcstore"})
	if err != nil {
		t.Fatal(err)
	}
	r := &fakeRunner{}
	prefix := filepath.Join(t.TempDir(), "pref ix")
	if err := Install(cs, Options{Prefix: prefix, Runner: r, SourceRoot: "/no/such/checkout"}); err != nil {
		t.Fatal(err)
	}
	if len(r.calls) != 1 {
		t.Fatalf("calls=%v", r.calls)
	}
	if !strings.Contains(r.calls[0], "--root") || !strings.Contains(r.calls[0], prefix) {
		t.Fatalf("expected cargo --root with spaces intact, got %q", r.calls[0])
	}
	if strings.Contains(r.calls[0], "pref") && !strings.Contains(r.calls[0], "pref ix") {
		t.Fatalf("prefix was word-split: %q", r.calls[0])
	}
}

func TestInstallSavesHostReceiptIfRustFails(t *testing.T) {
	src := t.TempDir()
	if err := osMkdirCheckout(src); err != nil {
		t.Fatal(err)
	}
	cs, err := Resolve([]string{"all"})
	if err != nil {
		t.Fatal(err)
	}
	r := &failingRunner{failSubstr: "install-components.sh"}
	prefix := t.TempDir()
	err = Install(cs, Options{Prefix: prefix, Runner: r, SourceRoot: src})
	if err == nil {
		t.Fatal("expected rust failure")
	}
	st := LoadState(prefix)
	if _, ok := st.Installed["host"]; !ok {
		t.Fatalf("host succeeded but receipt was dropped: %+v", st.Installed)
	}
}

func TestConcurrentSaveStateLeavesValidJSON(t *testing.T) {
	prefix := t.TempDir()
	ids := []string{"host", "devmap", "dcstore", "dcverify", "dcgrep"}
	var wg sync.WaitGroup
	for i := 0; i < 40; i++ {
		wg.Add(1)
		id := ids[i%len(ids)]
		go func() {
			defer wg.Done()
			st := LoadState(prefix)
			st.Record(id, filepath.Join(prefix, "bin", id))
			_ = SaveState(st)
		}()
	}
	wg.Wait()
	data, err := os.ReadFile(filepath.Join(prefix, "share", "devcouncil", "components.json"))
	if err != nil {
		t.Fatal(err)
	}
	var parsed State
	if err := json.Unmarshal(data, &parsed); err != nil {
		t.Fatalf("torn JSON after concurrent saves: %v\n%s", err, data)
	}
}

func TestResolveRejectsWhitespaceOnly(t *testing.T) {
	if _, err := Resolve([]string{"", "  ", "\t"}); err == nil {
		t.Fatal("expected no components selected")
	}
}

func TestDisableRejectsUnknownAndEmpty(t *testing.T) {
	prefix := t.TempDir()
	for _, id := range []string{"", "uv", "*", "..", "devmap/../dcgrep"} {
		if err := Disable(id, Options{Prefix: prefix}); err == nil {
			t.Fatalf("Disable(%q) should fail", id)
		}
		if err := Enable(id, Options{Prefix: prefix}); err == nil {
			t.Fatalf("Enable(%q) should fail", id)
		}
	}
}

type failingRunner struct {
	failSubstr string
	calls      []string
}

func (f *failingRunner) Run(name string, args []string, dir string, env []string) error {
	joined := name + " " + strings.Join(args, " ")
	f.calls = append(f.calls, joined)
	if f.failSubstr != "" && strings.Contains(joined, f.failSubstr) {
		return os.ErrPermission
	}
	return nil
}
