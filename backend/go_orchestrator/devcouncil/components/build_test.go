package components

import (
	"errors"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
)

// cargoRunner stands in for cargo: a `build -p PKG --bin BIN` writes
// "built BIN" where cargo would put it.
type cargoRunner struct {
	t     *testing.T
	src   string
	calls []string
	dirs  []string
}

func (r *cargoRunner) Run(name string, args []string, dir string, env []string) error {
	r.calls = append(r.calls, name+" "+strings.Join(args, " "))
	r.dirs = append(r.dirs, dir)
	if name != "cargo" {
		return nil
	}
	profile := "debug"
	if slices.Contains(args, "--release") {
		profile = "release"
	}
	bin := args[slices.Index(args, "--bin")+1]
	out := filepath.Join(cargoTargetDir(dir), profile, rustBinaryName(bin))
	if err := os.MkdirAll(filepath.Dir(out), 0o755); err != nil {
		r.t.Fatal(err)
	}
	return os.WriteFile(out, []byte("built "+bin), 0o755)
}

// healthyProber answers every health question the way a good build does.
type healthyProber struct{}

func (healthyProber) Probe(bin string, args []string) ([]byte, error) {
	id := strings.TrimSuffix(filepath.Base(bin), ".exe")
	joined := strings.Join(args, " ")
	switch {
	case strings.HasSuffix(joined, "health") && id == "dcstore":
		return []byte(`{"id":"dcstore","exclusion_index":"verified"}`), nil
	case joined == "health" && id == "dcverify":
		return []byte(`{"id":"dcverify","verifier":"dc-verify"}`), nil
	case joined == "health" && id == "dcgrep":
		return []byte(`{"id":"dcgrep","ok":true}`), nil
	}
	return []byte(`{"id":"` + id + `","version":"0.0.0"}`), nil
}

// sickProber is healthy except for one binary's health reply.
type sickProber struct {
	sick  string
	reply string
}

func (p sickProber) Probe(bin string, args []string) ([]byte, error) {
	if strings.TrimSuffix(filepath.Base(bin), ".exe") == p.sick && args[len(args)-1] == "health" {
		return []byte(p.reply), nil
	}
	return healthyProber{}.Probe(bin, args)
}

func noPath(string) (string, error) { return "", errors.New("not found") }

func checkout(t *testing.T) string {
	t.Helper()
	src := t.TempDir()
	if err := osMkdirCheckout(src); err != nil {
		t.Fatal(err)
	}
	t.Setenv("CARGO_TARGET_DIR", "")
	t.Setenv("PROFILE", "")
	return src
}

func plant(t *testing.T, prefix, id, body string) string {
	t.Helper()
	dest := filepath.Join(prefix, "bin", rustBinaryName(id))
	if err := os.MkdirAll(filepath.Dir(dest), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(dest, []byte(body), 0o755); err != nil {
		t.Fatal(err)
	}
	return dest
}

func TestAnUnhealthyBuildInstallsNothingAndKeepsTheOldBinaries(t *testing.T) {
	src := checkout(t)
	prefix := t.TempDir()
	oldStore := plant(t, prefix, "dcstore", "old dcstore")
	oldVerify := plant(t, prefix, "dcverify", "old dcverify")
	cs, err := Resolve([]string{"dcstore", "dcverify"})
	if err != nil {
		t.Fatal(err)
	}
	// dcverify is healthy and builds second; it must not be installed either,
	// because the check runs over every build before any install.
	err = Install(cs, Options{
		Prefix: prefix, SourceRoot: src, LookPath: noPath,
		Runner: &cargoRunner{t: t, src: src},
		Prober: sickProber{sick: "dcstore", reply: `{"id":"dcstore"}`},
	})
	if err == nil || !strings.Contains(err.Error(), "exclusion index") {
		t.Fatalf("err = %v", err)
	}
	for path, want := range map[string]string{oldStore: "old dcstore", oldVerify: "old dcverify"} {
		if got, _ := os.ReadFile(path); string(got) != want {
			t.Fatalf("%s = %q, want %q", path, got, want)
		}
	}
	if st := LoadState(prefix); len(st.Installed) != 0 {
		t.Fatalf("receipt records an install that did not happen: %+v", st.Installed)
	}
}

func TestHealthChecksRequireEachComponentsOwnAnswer(t *testing.T) {
	for _, tc := range []struct{ id, reply, want string }{
		{"dcstore", `{"exclusion_index":"verified"}`, "did not report id"},
		{"dcverify", `{"id":"dcverify"}`, "did not identify itself"},
		{"dcverify", `{"verifier":"dc-verify"}`, "did not report id"},
		{"dcgrep", `{"id":"dcgrep","ok":false}`, "health failed"},
		{"dcgrep", `{"ok":true}`, "did not report id"},
	} {
		err := healthCheck(tc.id, "/built/"+tc.id, sickProber{sick: tc.id, reply: tc.reply})
		if err == nil || !strings.Contains(err.Error(), tc.want) {
			t.Fatalf("%s %s: err = %v, want %q", tc.id, tc.reply, err, tc.want)
		}
	}
	for _, id := range []string{"dcstore", "dcverify", "dcgrep", "devmap"} {
		if err := healthCheck(id, "/built/"+id, healthyProber{}); err != nil {
			t.Fatalf("%s: %v", id, err)
		}
	}
}

type failingProber struct{}

func (failingProber) Probe(string, []string) ([]byte, error) {
	return []byte("boom"), errors.New("exit status 101")
}

func TestABinaryThatCannotAnswerFailsItsHealthCheck(t *testing.T) {
	err := healthCheck("devmap", "/built/devmap", failingProber{})
	if err == nil || !strings.Contains(err.Error(), "devmap --version failed") || !strings.Contains(err.Error(), "boom") {
		t.Fatalf("err = %v", err)
	}
}

func TestProfileSelectsTheBuildAndTheDirectoryItIsReadFrom(t *testing.T) {
	src := checkout(t)
	t.Setenv("PROFILE", "debug")
	cs, _ := Resolve([]string{"dcgrep"})
	r := &cargoRunner{t: t, src: src}
	prefix := t.TempDir()
	if err := Install(cs, Options{Prefix: prefix, SourceRoot: src, Runner: r, Prober: healthyProber{}, LookPath: noPath}); err != nil {
		t.Fatal(err)
	}
	if r.calls[0] != "cargo build -p dc-grep --bin dcgrep" {
		t.Fatalf("calls=%v", r.calls)
	}

	t.Setenv("PROFILE", "fast")
	if err := Install(cs, Options{Prefix: prefix, SourceRoot: src, Runner: r, Prober: healthyProber{}, LookPath: noPath}); err == nil || !strings.Contains(err.Error(), "PROFILE must be") {
		t.Fatalf("err = %v", err)
	}
}

func TestCargoTargetDirIsWhereTheBuildIsReadFrom(t *testing.T) {
	// A stale binary in rust/target must not be installed when the build
	// went to CARGO_TARGET_DIR.
	src := checkout(t)
	lane := t.TempDir()
	t.Setenv("CARGO_TARGET_DIR", lane)
	stale := filepath.Join(src, "rust", "target", "release", rustBinaryName("devmap"))
	if err := os.MkdirAll(filepath.Dir(stale), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(stale, []byte("stale"), 0o755); err != nil {
		t.Fatal(err)
	}
	cs, _ := Resolve([]string{"devmap"})
	prefix := t.TempDir()
	if err := Install(cs, Options{Prefix: prefix, SourceRoot: src, Runner: &cargoRunner{t: t, src: src}, Prober: healthyProber{}, LookPath: noPath}); err != nil {
		t.Fatal(err)
	}
	if got, _ := os.ReadFile(filepath.Join(prefix, "bin", rustBinaryName("devmap"))); string(got) != "built devmap" {
		t.Fatalf("installed %q", got)
	}
}

func TestAMissingBuildOutputIsAnError(t *testing.T) {
	src := checkout(t)
	cs, _ := Resolve([]string{"devmap"})
	err := Install(cs, Options{Prefix: t.TempDir(), SourceRoot: src, Runner: &fakeRunner{}, Prober: healthyProber{}, LookPath: noPath})
	if err == nil || !strings.Contains(err.Error(), "expected") {
		t.Fatalf("err = %v", err)
	}
}

func TestInstallAtomicReplacesWholeAndLeavesNoTemporary(t *testing.T) {
	dir := t.TempDir()
	src := filepath.Join(dir, "built")
	dest := filepath.Join(dir, "bin", "tool")
	if err := os.WriteFile(src, []byte("new"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(filepath.Dir(dest), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(dest, []byte("old"), 0o755); err != nil {
		t.Fatal(err)
	}
	// Hold the old inode open: a rename leaves it intact for its reader.
	held, err := os.Open(dest)
	if err != nil {
		t.Fatal(err)
	}
	defer held.Close()
	if err := installAtomic(src, dest); err != nil {
		t.Fatal(err)
	}
	if got, _ := os.ReadFile(dest); string(got) != "new" {
		t.Fatalf("dest = %q", got)
	}
	buf := make([]byte, 3)
	if _, err := held.Read(buf); err != nil || string(buf) != "old" {
		t.Fatalf("the running copy was rewritten underneath its reader: %q, %v", buf, err)
	}
	if _, err := os.Stat(dest + ".new"); !os.IsNotExist(err) {
		t.Fatalf("temporary left behind: %v", err)
	}
	if info, err := os.Stat(dest); err != nil || info.Mode().Perm()&0o100 == 0 {
		t.Fatalf("installed binary is not executable: %v %v", info, err)
	}
}

func TestResolutionNotesNameMissingAndShadowedBinaries(t *testing.T) {
	bindir := filepath.Join("/p", "bin")
	lookPath := func(name string) (string, error) {
		switch name {
		case "devmap":
			return filepath.Join(bindir, "devmap"), nil
		case "dcgrep":
			return "/usr/local/bin/dcgrep", nil
		}
		return "", errors.New("not found")
	}
	notes := resolutionNotes(bindir, []string{"devmap", "dcgrep", "dcstore"}, lookPath)
	if len(notes) != 2 ||
		!strings.Contains(notes[0], "dcgrep resolves to /usr/local/bin/dcgrep") ||
		!strings.Contains(notes[1], "dcstore is not on PATH") {
		t.Fatalf("notes=%v", notes)
	}
}

func TestLocalModelHintSizesTheModelToMemory(t *testing.T) {
	const gib = 1 << 30
	for _, tc := range []struct {
		ram  uint64
		want string
	}{{16 * gib, "qwen2.5-coder:7b"}, {24 * gib, "qwen2.5-coder:14b"}, {64 * gib, "qwen2.5-coder:32b"}} {
		if hint := localModelHint(tc.ram, true, true); !strings.Contains(hint, tc.want) || strings.Contains(hint, "brew install") {
			t.Fatalf("%d GiB: %s", tc.ram/gib, hint)
		}
	}
	if hint := localModelHint(8*gib, false, false); !strings.Contains(hint, "Intel") || !strings.Contains(hint, "brew install ollama") {
		t.Fatalf("hint=%s", hint)
	}
}

func TestAPartialInstallRecordsWhatItPutOnDisk(t *testing.T) {
	// dcstore copies; dcverify's destination is a directory, so its rename
	// fails. The receipt must name dcstore, or uninstall cannot find it.
	src := checkout(t)
	prefix := t.TempDir()
	blocker := filepath.Join(prefix, "bin", rustBinaryName("dcverify"))
	if err := os.MkdirAll(blocker, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(blocker, "keep"), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	cs, _ := Resolve([]string{"dcstore", "dcverify"})
	err := Install(cs, Options{Prefix: prefix, SourceRoot: src, Runner: &cargoRunner{t: t, src: src}, Prober: healthyProber{}, LookPath: noPath})
	if err == nil || !strings.Contains(err.Error(), "install dcverify") {
		t.Fatalf("err = %v", err)
	}
	st := LoadState(prefix)
	if _, ok := st.Installed["dcstore"]; !ok || len(st.Installed) != 1 {
		t.Fatalf("receipt = %+v, want exactly dcstore", st.Installed)
	}
	if got, _ := os.ReadFile(filepath.Join(prefix, "bin", rustBinaryName("dcstore"))); string(got) != "built dcstore" {
		t.Fatalf("dcstore = %q", got)
	}
}
