package main

import (
	"bytes"
	"io"
	"os"
	"path/filepath"
	"reflect"
	"runtime"
	"strings"
	"sync"
	"testing"
)

func TestDispatchInstallHelp(t *testing.T) {
	stdout, restoreOut := swapStdout(t)
	stderr, restoreErr := swapStderr(t)
	code := dispatch([]string{"install", "--help"})
	restoreErr()
	restoreOut()
	if code != 0 {
		t.Fatalf("exit %d stderr=%s", code, stderr.String())
	}
	got := stdout.String() + stderr.String()
	if !strings.Contains(got, "devmap") || !strings.Contains(got, "--list") {
		t.Fatalf("help = %q", got)
	}
}

func TestDispatchUninstallWithoutNamesRefuses(t *testing.T) {
	prefix := t.TempDir()
	stderr, restore := swapStderr(t)
	code := dispatch([]string{"uninstall", "--yes", "--prefix", prefix})
	restore()
	if code != 2 {
		t.Fatalf("empty uninstall exit %d want 2 stderr=%s", code, stderr.String())
	}
}

func TestDispatchUninstallMixesAllWithNames(t *testing.T) {
	prefix := t.TempDir()
	stderr, restore := swapStderr(t)
	code := dispatch([]string{"uninstall", "--all", "dcgrep", "--yes", "--prefix", prefix})
	restore()
	if code != 2 {
		t.Fatalf("mixed --all exit %d want 2 stderr=%s", code, stderr.String())
	}
}

func TestDispatchUninstallAllRequiresYes(t *testing.T) {
	prefix := t.TempDir()
	stderr, restore := swapStderr(t)
	code := dispatch([]string{"uninstall", "--all", "--prefix", prefix})
	restore()
	if code != 2 {
		t.Fatalf("uninstall --all without --yes exit %d want 2 stderr=%s", code, stderr.String())
	}
}

func TestDispatchInstallPrefixEquals(t *testing.T) {
	stdout, restoreOut := swapStdout(t)
	stderr, restoreErr := swapStderr(t)
	code := dispatch([]string{"install", "--list", "--json", "--prefix=/does/not/matter"})
	restoreOut()
	restoreErr()
	if code != 0 {
		t.Fatalf("exit %d stderr=%s", code, stderr.String())
	}
	if !strings.Contains(stdout.String(), `"id": "devmap"`) {
		t.Fatalf("stdout=%s", stdout.String())
	}
}

func TestDispatchInstallUnknownComponent(t *testing.T) {
	stderr, restore := swapStderr(t)
	code := dispatch([]string{"install", "uv"})
	restore()
	if code != 2 {
		t.Fatalf("exit %d, want 2", code)
	}
	if !strings.Contains(stderr.String(), "unknown") {
		t.Fatalf("stderr=%q", stderr.String())
	}
}

func TestDispatchGateStatusDefaultsOff(t *testing.T) {
	root := t.TempDir()
	stdout, restoreOut := swapStdout(t)
	stderr, restoreErr := swapStderr(t)
	code := dispatch([]string{"gate", "status", "--json", "--project-root", root})
	restoreErr()
	restoreOut()
	if code != 0 {
		t.Fatalf("exit %d stderr=%s stdout=%s", code, stderr.String(), stdout.String())
	}
	if !strings.Contains(stdout.String(), `"verification_mode": "off"`) {
		t.Fatalf("stdout=%s", stdout.String())
	}
}

func TestDispatchGateHelp(t *testing.T) {
	stderr, restore := swapStderr(t)
	code := dispatch([]string{"gate", "--help"})
	restore()
	if code != 0 {
		t.Fatalf("exit %d", code)
	}
	if !strings.Contains(stderr.String(), "advisory") {
		t.Fatalf("stderr=%q", stderr.String())
	}
}

func TestUnknownCommandExit2(t *testing.T) {
	for _, name := range []string{"hook", "boot", "status", "init", "doctor"} {
		t.Run(name, func(t *testing.T) {
			stderr, restore := swapStderr(t)
			code := dispatch([]string{name})
			restore()
			if code != 2 {
				t.Fatalf("dispatch(%q) exit %d, want 2", name, code)
			}
			if !strings.Contains(stderr.String(), "unknown command: "+name) {
				t.Fatalf("stderr = %q, want unknown command: %s", stderr.String(), name)
			}
		})
	}
}

func TestNoArgsExit2(t *testing.T) {
	_, restore := swapStderr(t)
	code := dispatch(nil)
	restore()
	if code != 2 {
		t.Fatalf("dispatch(nil) exit %d, want 2", code)
	}
}

func TestIntegrationsIsIntegrateAlias(t *testing.T) {
	stderr, restore := swapStderr(t)
	code := dispatch([]string{"integrations"})
	restore()
	if code != 2 {
		t.Fatalf("exit %d, want 2 (missing host)", code)
	}
	got := stderr.String()
	if strings.Contains(got, "unknown command") {
		t.Fatalf("integrations was rejected as unknown: %s", got)
	}
	if !strings.Contains(got, "integrate requires a host") {
		t.Fatalf("stderr = %q", got)
	}
}

func TestDispatchMapForwardsArgv(t *testing.T) {
	_, argvFile := fakeDevmap(t)
	if code := dispatch([]string{"map", "--json", "paths"}); code != 0 {
		t.Fatalf("exit %d", code)
	}
	got := readArgv(t, argvFile)
	want := []string{"--json", "paths"}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("argv = %v, want %v", got, want)
	}
}

func TestDispatchMapBareIsBuildManifest(t *testing.T) {
	_, argvFile := fakeDevmap(t)
	if code := dispatch([]string{"map"}); code != 0 {
		t.Fatalf("exit %d", code)
	}
	got := readArgv(t, argvFile)
	want := []string{"build", "--manifest"}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("argv = %v, want %v", got, want)
	}
}

func TestDispatchMapJSONInsertsDefault(t *testing.T) {
	_, argvFile := fakeDevmap(t)
	if code := dispatch([]string{"map", "--json"}); code != 0 {
		t.Fatalf("exit %d", code)
	}
	got := readArgv(t, argvFile)
	want := []string{"--json", "build", "--manifest"}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("argv = %v, want %v", got, want)
	}
}

func TestDispatchMapStatus(t *testing.T) {
	_, argvFile := fakeDevmap(t)
	if code := dispatch([]string{"map", "status"}); code != 0 {
		t.Fatalf("exit %d", code)
	}
	got := readArgv(t, argvFile)
	want := []string{"status"}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("argv = %v, want %v", got, want)
	}
}

func TestDispatchGraphIsMapAlias(t *testing.T) {
	_, argvFile := fakeDevmap(t)
	if code := dispatch([]string{"graph"}); code != 0 {
		t.Fatalf("exit %d", code)
	}
	got := readArgv(t, argvFile)
	want := []string{"build", "--manifest"}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("argv = %v, want %v", got, want)
	}
}

func TestDispatchAstPrependsAst(t *testing.T) {
	_, argvFile := fakeDevmap(t)
	if code := dispatch([]string{"ast", "--json", "Foo"}); code != 0 {
		t.Fatalf("exit %d", code)
	}
	got := readArgv(t, argvFile)
	want := []string{"ast", "--json", "Foo"}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("argv = %v, want %v", got, want)
	}
}

func TestEmptyDevmapBinLooksUpPATH(t *testing.T) {
	bin, argvFile := fakeDevmap(t)
	t.Setenv("DEVMAP_BIN", "")
	t.Setenv("PATH", filepath.Dir(bin))
	if code := dispatch([]string{"map", "status"}); code != 0 {
		t.Fatalf("exit %d", code)
	}
	got := readArgv(t, argvFile)
	want := []string{"status"}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("argv = %v, want %v", got, want)
	}
}

func TestMissingDevmapExit1(t *testing.T) {
	t.Setenv("DEVMAP_BIN", "")
	t.Setenv("HOME", t.TempDir())
	t.Setenv("PATH", t.TempDir())
	stderr, restore := swapStderr(t)
	code := dispatch([]string{"map"})
	restore()
	if code != 1 {
		t.Fatalf("exit %d, want 1", code)
	}
	if !strings.Contains(stderr.String(), "devmap binary not found") {
		t.Fatalf("stderr = %q", stderr.String())
	}
}

func TestDevmapBinMissingIsRefusedNotReplaced(t *testing.T) {
	other, argvFile := fakeDevmap(t)
	missing := filepath.Join(t.TempDir(), "no-such-devmap")
	t.Setenv("DEVMAP_BIN", missing)
	t.Setenv("PATH", filepath.Dir(other))
	stderr, restore := swapStderr(t)
	code := dispatch([]string{"map"})
	restore()
	if code != 1 {
		t.Fatalf("exit %d, want 1", code)
	}
	if _, err := os.Stat(argvFile); !os.IsNotExist(err) {
		t.Fatalf("PATH binary was used as a replacement (argv file exists, err=%v)", err)
	}
	got := stderr.String()
	if !strings.Contains(got, "DEVMAP_BIN") {
		t.Fatalf("stderr = %q, want DEVMAP_BIN", got)
	}
}

func TestDevmapBinDirectoryIsRefused(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("DEVMAP_BIN", dir)
	t.Setenv("HOME", t.TempDir())
	t.Setenv("PATH", t.TempDir())
	stderr, restore := swapStderr(t)
	code := dispatch([]string{"map"})
	restore()
	if code != 1 {
		t.Fatalf("exit %d, want 1", code)
	}
	if !strings.Contains(stderr.String(), "directory") {
		t.Fatalf("stderr = %q", stderr.String())
	}
}

func TestIntegrateUninstallTargetHooks(t *testing.T) {
	root := t.TempDir()
	hooks := filepath.Join(root, ".cursor", "hooks.json")
	if err := os.MkdirAll(filepath.Dir(hooks), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(hooks, []byte(`{"version":1}`), 0o644); err != nil {
		t.Fatal(err)
	}
	stdout, restoreOut := swapStdout(t)
	stderr, restoreErr := swapStderr(t)
	code := dispatch([]string{"integrate", "uninstall", "--target", "hooks", "--project-root", root})
	restoreErr()
	restoreOut()
	if code != 0 {
		t.Fatalf("exit %d\nstderr=%s\nstdout=%s", code, stderr.String(), stdout.String())
	}
	if _, err := os.Stat(hooks); !os.IsNotExist(err) {
		t.Fatalf("hooks.json still present (stat err %v)", err)
	}
	if !strings.Contains(stdout.String(), "removed") {
		t.Fatalf("receipt = %s", stdout.String())
	}
}

func fakeDevmap(t *testing.T) (bin, argvFile string) {
	t.Helper()
	if runtime.GOOS == "windows" {
		t.Skip("fake devmap is a shell script")
	}
	dir := t.TempDir()
	argvFile = filepath.Join(dir, "argv")
	bin = filepath.Join(dir, "devmap")
	script := "#!/bin/sh\n: > \"$DEVMAP_ARGV_FILE\"\nfor a in \"$@\"; do printf '%s\\n' \"$a\" >> \"$DEVMAP_ARGV_FILE\"; done\n"
	if err := os.WriteFile(bin, []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("DEVMAP_ARGV_FILE", argvFile)
	t.Setenv("DEVMAP_BIN", bin)
	t.Setenv("HOME", t.TempDir())
	return bin, argvFile
}

func readArgv(t *testing.T, path string) []string {
	t.Helper()
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	s := strings.TrimSuffix(string(data), "\n")
	if s == "" {
		return nil
	}
	return strings.Split(s, "\n")
}

func swapStderr(t *testing.T) (*bytes.Buffer, func()) {
	t.Helper()
	return swapWriter(t, &os.Stderr)
}

func swapStdout(t *testing.T) (*bytes.Buffer, func()) {
	t.Helper()
	return swapWriter(t, &os.Stdout)
}

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
	var once sync.Once
	restore := func() {
		once.Do(func() {
			_ = w.Close()
			<-done
			*slot = old
			_ = r.Close()
		})
	}
	t.Cleanup(restore)
	return buf, restore
}
