package components

import (
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

func repoRoot(t *testing.T) string {
	t.Helper()
	wd, err := os.Getwd()
	if err != nil {
		t.Fatal(err)
	}
	for dir := wd; ; {
		if looksLikeCheckout(dir) {
			return dir
		}
		parent := filepath.Dir(dir)
		if parent == dir {
			t.Skip("not running from a DevCouncil checkout")
		}
		dir = parent
	}
}

func TestInstallScriptsHelp(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("posix scripts")
	}
	root := repoRoot(t)
	for _, name := range []string{"install.sh", "install-components.sh"} {
		path := filepath.Join(root, "scripts", name)
		out, err := exec.Command("bash", path, "--help").CombinedOutput()
		if err != nil {
			t.Fatalf("%s --help: %v\n%s", name, err, out)
		}
		text := string(out)
		if !strings.Contains(text, "Usage") && !strings.Contains(text, "usage") {
			t.Fatalf("%s --help missing usage:\n%s", name, text)
		}
		if name == "install.sh" && !strings.Contains(text, "--only") {
			t.Fatalf("install.sh --help must document --only:\n%s", text)
		}
		if name == "install-components.sh" && !strings.Contains(text, "devmap") {
			t.Fatalf("install-components.sh --help must name devmap:\n%s", text)
		}
	}
}

func TestInstallShUnknownComponent(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("posix scripts")
	}
	root := repoRoot(t)
	cmd := exec.Command("bash", filepath.Join(root, "scripts", "install.sh"), "uv")
	out, err := cmd.CombinedOutput()
	if err == nil {
		t.Fatalf("uv should be unknown:\n%s", out)
	}
	if !strings.Contains(string(out), "unknown") {
		t.Fatalf("stderr=%s", out)
	}
}

func TestInstallShUninstallRequiresName(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("posix scripts")
	}
	root := repoRoot(t)
	prefix := t.TempDir()
	cmd := exec.Command("bash", filepath.Join(root, "scripts", "install.sh"), "--uninstall", "--yes")
	cmd.Env = append(os.Environ(), "PREFIX="+prefix)
	out, err := cmd.CombinedOutput()
	if err == nil {
		t.Fatalf("uninstall with no names should refuse:\n%s", out)
	}
}

func TestInstallShUninstallAllDryRunNamesRustBinaries(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("posix scripts")
	}
	root := repoRoot(t)
	prefix := t.TempDir()
	cmd := exec.Command("bash", filepath.Join(root, "scripts", "install.sh"), "--uninstall", "--all", "--dry-run", "--yes")
	cmd.Env = append(os.Environ(), "PREFIX="+prefix)
	out, err := cmd.CombinedOutput()
	if err != nil {
		t.Fatalf("%v\n%s", err, out)
	}
	text := string(out)
	for _, name := range []string{"devcouncil", "devmap", "dcstore", "dcverify", "dcgrep"} {
		if !strings.Contains(text, name) {
			t.Fatalf("--all dry-run missing %s:\n%s", name, text)
		}
	}
}

func TestInstallShUninstallMixesAllWithNames(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("posix scripts")
	}
	root := repoRoot(t)
	cmd := exec.Command("bash", filepath.Join(root, "scripts", "install.sh"), "--uninstall", "--all", "dcgrep", "--yes")
	out, err := cmd.CombinedOutput()
	if err == nil {
		t.Fatalf("mix should refuse:\n%s", out)
	}
}

func TestInstallComponentsUninstallRequiresName(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("posix scripts")
	}
	root := repoRoot(t)
	prefix := t.TempDir()
	cmd := exec.Command("bash", filepath.Join(root, "scripts", "install-components.sh"), "--uninstall", "--yes")
	cmd.Env = append(os.Environ(), "PREFIX="+prefix)
	out, err := cmd.CombinedOutput()
	if err == nil {
		t.Fatalf("empty uninstall should refuse:\n%s", out)
	}
}

func TestInstallShUninstallDryRunDoesNotDelete(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("posix scripts")
	}
	root := repoRoot(t)
	prefix := t.TempDir()
	bindir := filepath.Join(prefix, "bin")
	if err := os.MkdirAll(bindir, 0o755); err != nil {
		t.Fatal(err)
	}
	planted := filepath.Join(bindir, "devmap")
	if err := os.WriteFile(planted, []byte("keep"), 0o755); err != nil {
		t.Fatal(err)
	}
	cmd := exec.Command("bash", filepath.Join(root, "scripts", "install.sh"), "--uninstall", "devmap", "--dry-run", "--yes")
	cmd.Env = append(os.Environ(), "PREFIX="+prefix)
	out, err := cmd.CombinedOutput()
	if err != nil {
		t.Fatalf("%v\n%s", err, out)
	}
	if _, err := os.Stat(planted); err != nil {
		t.Fatalf("dry-run deleted %s: %v", planted, err)
	}
}

func TestInstallComponentsDisableValidatesName(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("posix scripts")
	}
	root := repoRoot(t)
	prefix := t.TempDir()
	script := filepath.Join(root, "scripts", "install-components.sh")
	cmd := exec.Command("bash", script, "--disable", "uv")
	cmd.Env = append(os.Environ(), "PREFIX="+prefix)
	out, err := cmd.CombinedOutput()
	if err == nil {
		t.Fatalf("disable uv should fail:\n%s", out)
	}

	cmd = exec.Command("bash", script, "--disable")
	cmd.Env = append(os.Environ(), "PREFIX="+prefix)
	out, err = cmd.CombinedOutput()
	if err == nil {
		t.Fatalf("disable without a name should fail:\n%s", out)
	}
}

func TestInstallShDryRunDoesNotClaimInstalled(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("posix scripts")
	}
	root := repoRoot(t)
	prefix := t.TempDir()
	cmd := exec.Command("bash", filepath.Join(root, "scripts", "install.sh"), "--all", "--dry-run")
	cmd.Env = append(os.Environ(), "PREFIX="+prefix)
	out, err := cmd.CombinedOutput()
	if err != nil {
		t.Fatalf("%v\n%s", err, out)
	}
	text := string(out)
	if strings.Contains(text, "installed "+prefix) {
		t.Fatalf("dry-run claimed an install:\n%s", text)
	}
	if _, err := os.Stat(filepath.Join(prefix, "bin", "devcouncil")); err == nil {
		t.Fatal("dry-run wrote the host binary")
	}
}

func TestInstallShPrefixRequiresDirectory(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("posix scripts")
	}
	root := repoRoot(t)
	cmd := exec.Command("bash", filepath.Join(root, "scripts", "install.sh"), "--prefix")
	out, err := cmd.CombinedOutput()
	if err == nil {
		t.Fatalf("--prefix without a directory should fail:\n%s", out)
	}
}
