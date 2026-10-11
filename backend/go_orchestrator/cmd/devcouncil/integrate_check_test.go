package main

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// integrateEnv is a repository and a HOME that belong to the test. Nothing here
// may reach the real ~/.codex, ~/.cursor or ~/.claude.json: the real devmap
// binary is driven, and it writes user-level host documents under $HOME.
func integrateEnv(t *testing.T) (home, repo string) {
	t.Helper()
	// Built before HOME moves: cargo and rustup find their toolchain there.
	devmapBin := testsupport.Devmap(t)
	home = t.TempDir()
	repo = t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("DEVMAP_BIN", devmapBin)
	if out, err := exec.Command("git", "-C", repo, "init", "-q").CombinedOutput(); err != nil {
		t.Fatalf("git init: %v: %s", err, out)
	}
	return home, repo
}

func runDispatch(t *testing.T, args ...string) (code int, stdout, stderr string) {
	t.Helper()
	out, restoreOut := swapStdout(t)
	errw, restoreErr := swapStderr(t)
	code = dispatch(args)
	restoreErr()
	restoreOut()
	return code, out.String(), errw.String()
}

// `--check` is how a script asks "is this installed?". It exited 0 for every
// host on a tree with nothing installed, because the report printed and the
// command returned success regardless of what the report said.
func TestIntegrateCheckExitsOneOnDrift(t *testing.T) {
	_, repo := integrateEnv(t)
	code, stdout, stderr := runDispatch(t, "integrate", "claude", "--check", "--project-root", repo)
	if code != 1 {
		t.Fatalf("exit %d, want 1 (drift)\nstdout=%s\nstderr=%s", code, stdout, stderr)
	}
	if !strings.Contains(stdout, `"drift"`) && !strings.Contains(stdout, `"missing"`) {
		t.Fatalf("the receipt must still be printed:\n%s", stdout)
	}
}

func TestIntegrateCheckExitsZeroOnceInstalled(t *testing.T) {
	_, repo := integrateEnv(t)
	if code, stdout, stderr := runDispatch(t, "integrate", "claude", "--apply", "--project-root", repo); code != 0 {
		t.Fatalf("apply: exit %d\nstdout=%s\nstderr=%s", code, stdout, stderr)
	}
	code, stdout, stderr := runDispatch(t, "integrate", "claude", "--check", "--project-root", repo)
	if code != 0 {
		t.Fatalf("check after apply: exit %d\nstdout=%s\nstderr=%s", code, stdout, stderr)
	}
}

// A document devmap refuses is not "drift": devmap's error envelope was decoded
// as a report with check_ok false, so the refused file vanished from the receipt
// and the command exited 0. It is its own exit code, and it names the file.
func TestIntegrateCheckDistinguishesARefusalFromDrift(t *testing.T) {
	home, repo := integrateEnv(t)
	cursorDir := filepath.Join(home, ".cursor")
	if err := os.MkdirAll(cursorDir, 0o755); err != nil {
		t.Fatal(err)
	}
	jsonc := filepath.Join(cursorDir, "mcp.json")
	if err := os.WriteFile(jsonc, []byte("// my servers\n{\"mcpServers\":{}}\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	code, stdout, stderr := runDispatch(t, "integrate", "cursor", "--check", "--project-root", repo)
	if code != exitDevmapRefused {
		t.Fatalf("exit %d, want %d (devmap refused)\nstdout=%s\nstderr=%s", code, exitDevmapRefused, stdout, stderr)
	}
	if !strings.Contains(stderr, "mcp.json") {
		t.Fatalf("the refusal must name the failing path:\n%s", stderr)
	}
	if strings.Contains(stdout, `"drift"`) {
		t.Fatalf("a refusal must not be presented as drift:\n%s", stdout)
	}
}

func TestIntegrateHelpExitsZeroAndConflictingModesAreRefused(t *testing.T) {
	integrateEnv(t)
	code, _, stderr := runDispatch(t, "integrate", "--help")
	if code != 0 || !strings.Contains(stderr, "devcouncil integrate HOST") {
		t.Fatalf("integrate --help: exit %d, stderr=%q", code, stderr)
	}
	code, _, stderr = runDispatch(t, "integrate", "claude", "--help")
	if code != 0 {
		t.Fatalf("integrate HOST --help: exit %d, stderr=%q", code, stderr)
	}
	for _, flags := range [][]string{{"--apply", "--check"}, {"--check", "--apply"}, {"--dry-run", "--apply"}} {
		code, _, stderr = runDispatch(t, append([]string{"integrate", "claude"}, flags...)...)
		if code != 2 || !strings.Contains(stderr, "conflicting") {
			t.Fatalf("%v: exit %d, stderr=%q; want 2 and a conflict message", flags, code, stderr)
		}
	}
	// Repeating one mode is not a conflict.
	if code, _, stderr = runDispatch(t, "integrate", "claude", "--check", "--check", "--project-root", t.TempDir()); code == 2 {
		t.Fatalf("a repeated mode flag was refused: %s", stderr)
	}
}

// The way back: --remove follows the apply's receipt, defaults to a check, and
// leaves the user's own configuration alone.
func TestIntegrateRemoveUndoesAnApplyThroughTheCLI(t *testing.T) {
	home, repo := integrateEnv(t)
	userConfig := "# my codex config\nmodel = \"o5\"   # trailing comment\n"
	codexConfig := filepath.Join(home, ".codex", "config.toml")
	if err := os.MkdirAll(filepath.Dir(codexConfig), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(codexConfig, []byte(userConfig), 0o644); err != nil {
		t.Fatal(err)
	}
	if code, stdout, stderr := runDispatch(t, "integrate", "codex", "--apply", "--project-root", repo); code != 0 {
		t.Fatalf("apply: exit %d\nstdout=%s\nstderr=%s", code, stdout, stderr)
	}
	registered, _ := os.ReadFile(codexConfig)
	if !strings.Contains(string(registered), "[mcp_servers.devmap]") || !strings.HasPrefix(string(registered), userConfig) {
		t.Fatalf("apply did not register beside the user's lines:\n%s", registered)
	}

	// Default is a check: nothing is removed and the exit says it is still there.
	if code, stdout, stderr := runDispatch(t, "integrate", "codex", "--remove", "--project-root", repo); code != 1 {
		t.Fatalf("bare --remove: exit %d, want 1 (still registered)\nstdout=%s\nstderr=%s", code, stdout, stderr)
	}
	if got, _ := os.ReadFile(codexConfig); string(got) != string(registered) {
		t.Fatal("a check removed something")
	}

	if code, stdout, stderr := runDispatch(t, "integrate", "codex", "--remove", "--apply", "--project-root", repo); code != 0 {
		t.Fatalf("remove: exit %d\nstdout=%s\nstderr=%s", code, stdout, stderr)
	}
	if got, _ := os.ReadFile(codexConfig); string(got) != userConfig {
		t.Fatalf("the user's config did not come back byte for byte:\n%s", got)
	}
	// The receipt is spent: a second remove has nothing to follow.
	if code, _, stderr := runDispatch(t, "integrate", "codex", "--remove", "--apply", "--project-root", repo); code != 1 || !strings.Contains(stderr, "no apply receipt") {
		t.Fatalf("second remove: exit %d, stderr=%q", code, stderr)
	}
}
