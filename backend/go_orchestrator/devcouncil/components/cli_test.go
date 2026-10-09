package components

// The flag contract scripts/install.sh used to own. The scripts are now
// bootstraps that hand their arguments to these commands (through
// cmd/dcinstall), so each assertion their shell tests made lives here, beside
// the code that answers it.

import (
	"bytes"
	"io"
	"os"
	"path/filepath"
	"reflect"
	"runtime"
	"strings"
	"testing"
)

func swapWriter(t *testing.T, slot **os.File) (*bytes.Buffer, func()) {
	t.Helper()
	buf := new(bytes.Buffer)
	r, w, err := os.Pipe()
	if err != nil {
		t.Fatal(err)
	}
	old := *slot
	*slot = w
	done := make(chan struct{})
	go func() {
		_, _ = io.Copy(buf, r)
		close(done)
	}()
	return buf, func() {
		*slot = old
		_ = w.Close()
		<-done
		_ = r.Close()
	}
}

func swapStdout(t *testing.T) (*bytes.Buffer, func()) { return swapWriter(t, &os.Stdout) }
func swapStderr(t *testing.T) (*bytes.Buffer, func()) { return swapWriter(t, &os.Stderr) }

func TestParseInstallArgsKeepsTheOldScriptsSpellings(t *testing.T) {
	_, _, _, _, names, _, err := parseInstallArgs([]string{"--host-only", "-y", "--yes", "--", "devmap", "--prefix"})
	if err != nil {
		t.Fatal(err)
	}
	// --yes is accepted and means nothing to install; after -- every word is a name.
	if !reflect.DeepEqual(names, []string{"host", "devmap", "--prefix"}) {
		t.Fatalf("names=%v", names)
	}
}

func TestParseInstallArgsTakesOnlyAsAPositionalName(t *testing.T) {
	_, _, _, _, names, _, err := parseInstallArgs([]string{"--only", "devmap", "--only=dcgrep", "dcstore"})
	if err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(names, []string{"devmap", "dcgrep", "dcstore"}) {
		t.Fatalf("names=%v", names)
	}
	for _, argv := range [][]string{{"--only"}, {"--only="}, {"--prefix"}, {"--prefix="}, {"--bogus"}} {
		if _, _, _, _, _, _, err := parseInstallArgs(argv); err == nil {
			t.Fatalf("%v accepted", argv)
		}
	}
}

func TestInstallRefusesAnUnknownComponent(t *testing.T) {
	_, restore := swapStderr(t)
	code := RunInstall([]string{"uv", "--prefix", t.TempDir()})
	restore()
	if code != 2 {
		t.Fatalf("exit %d", code)
	}
}

func TestInstallPrefixRequiresADirectory(t *testing.T) {
	_, restore := swapStderr(t)
	code := RunInstall([]string{"--prefix"})
	restore()
	if code != 2 {
		t.Fatalf("exit %d", code)
	}
}

func TestInstallDryRunWritesNothing(t *testing.T) {
	prefix := t.TempDir()
	stdout, restore := swapStdout(t)
	code := RunInstall([]string{"all", "--dry-run", "--prefix", prefix})
	restore()
	if code != 0 {
		t.Fatalf("exit %d: %s", code, stdout)
	}
	if strings.Contains(stdout.String(), "installed ") {
		t.Fatalf("dry-run claimed an install:\n%s", stdout)
	}
	entries, err := os.ReadDir(prefix)
	if err != nil {
		t.Fatal(err)
	}
	if len(entries) != 0 {
		t.Fatalf("dry-run wrote under the prefix: %v", entries)
	}
}

func TestUninstallRequiresANameOrAll(t *testing.T) {
	_, restore := swapStderr(t)
	code := RunUninstall([]string{"--yes", "--prefix", t.TempDir()})
	restore()
	if code != 2 {
		t.Fatalf("exit %d", code)
	}
}

func TestUninstallRefusesAllMixedWithNames(t *testing.T) {
	_, restore := swapStderr(t)
	code := RunUninstall([]string{"--all", "dcgrep", "--yes", "--prefix", t.TempDir()})
	restore()
	if code != 2 {
		t.Fatalf("exit %d", code)
	}
}

func plantCatalog(t *testing.T, prefix string) map[string]string {
	t.Helper()
	bindir := filepath.Join(prefix, "bin")
	if err := os.MkdirAll(bindir, 0o755); err != nil {
		t.Fatal(err)
	}
	ext := ""
	if runtime.GOOS == "windows" {
		ext = ".exe"
	}
	planted := map[string]string{}
	for _, name := range []string{"devcouncil", "devmap", "dcstore", "dcverify", "dcgrep"} {
		path := filepath.Join(bindir, name+ext)
		if err := os.WriteFile(path, []byte("keep"), 0o755); err != nil {
			t.Fatal(err)
		}
		planted[name] = path
	}
	return planted
}

func TestUninstallAllDryRunNamesEveryBinaryAndDeletesNone(t *testing.T) {
	prefix := t.TempDir()
	planted := plantCatalog(t, prefix)
	stdout, restore := swapStdout(t)
	code := RunUninstall([]string{"--all", "--dry-run", "--yes", "--prefix", prefix})
	restore()
	if code != 0 {
		t.Fatalf("exit %d: %s", code, stdout)
	}
	for name, path := range planted {
		if !strings.Contains(stdout.String(), path) {
			t.Fatalf("--all dry-run did not name %s:\n%s", name, stdout)
		}
		if _, err := os.Stat(path); err != nil {
			t.Fatalf("dry-run deleted %s: %v", path, err)
		}
	}
}

func TestUninstallWithoutYesRefusesAndDeletesNothing(t *testing.T) {
	prefix := t.TempDir()
	planted := plantCatalog(t, prefix)
	_, restore := swapStderr(t)
	code := RunUninstall([]string{"devmap", "--prefix", prefix})
	restore()
	if code != 2 {
		t.Fatalf("exit %d", code)
	}
	if _, err := os.Stat(planted["devmap"]); err != nil {
		t.Fatalf("unconfirmed uninstall deleted devmap: %v", err)
	}
}
