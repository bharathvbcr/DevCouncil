package main

// The flag contract scripts/install.sh used to own. The script is now a
// bootstrap that hands its arguments to these commands, so each assertion
// its shell tests made moved here, to the code that answers them.

import (
	"os"
	"path/filepath"
	"reflect"
	"runtime"
	"strings"
	"testing"
)

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
	code := runInstall([]string{"uv", "--prefix", t.TempDir()})
	restore()
	if code != 2 {
		t.Fatalf("exit %d", code)
	}
}

func TestInstallPrefixRequiresADirectory(t *testing.T) {
	_, restore := swapStderr(t)
	code := runInstall([]string{"--prefix"})
	restore()
	if code != 2 {
		t.Fatalf("exit %d", code)
	}
}

func TestInstallDryRunWritesNothing(t *testing.T) {
	prefix := t.TempDir()
	stdout, restore := swapStdout(t)
	code := runInstall([]string{"all", "--dry-run", "--prefix", prefix})
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
	code := runUninstall([]string{"--yes", "--prefix", t.TempDir()})
	restore()
	if code != 2 {
		t.Fatalf("exit %d", code)
	}
}

func TestUninstallRefusesAllMixedWithNames(t *testing.T) {
	_, restore := swapStderr(t)
	code := runUninstall([]string{"--all", "dcgrep", "--yes", "--prefix", t.TempDir()})
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
	code := runUninstall([]string{"--all", "--dry-run", "--yes", "--prefix", prefix})
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
	code := runUninstall([]string{"devmap", "--prefix", prefix})
	restore()
	if code != 2 {
		t.Fatalf("exit %d", code)
	}
	if _, err := os.Stat(planted["devmap"]); err != nil {
		t.Fatalf("unconfirmed uninstall deleted devmap: %v", err)
	}
}

func TestDisableRefusesAnUnknownComponentAndAMissingName(t *testing.T) {
	prefix := t.TempDir()
	_, restore := swapStderr(t)
	unknown := runDisable([]string{"uv", "--prefix", prefix})
	missing := runDisable([]string{"--prefix", prefix})
	restore()
	if unknown == 0 || missing == 0 {
		t.Fatalf("disable uv exited %d, disable with no name exited %d", unknown, missing)
	}
}
