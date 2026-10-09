package components

// scripts/install.sh is a bootstrap: it builds the host and hands its
// arguments to `devcouncil install`. What it decides on its own — that Go is
// required, that --uninstall picks the verb, that the checkout is named, that
// the host's exit status is its own — is tested here against a stand-in `go`.
// What the installer does with the arguments is tested where it is decided:
// install_test.go, build_test.go and cmd/devcouncil/install_test.go.

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

// fakeGo is a `go` whose `build -o OUT` writes a host that prints
// DEVCOUNCIL_ROOT and then each argument on its own line, and exits with
// $FAKE_HOST_EXIT. It records its own arguments in $FAKE_GO_LOG.
const fakeGo = `#!/bin/sh
printf '%s\n' "$@" > "$FAKE_GO_LOG"
out=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = -o ]; then out="$2"; fi
  shift
done
[ -n "$out" ] || exit 9
cat > "$out" <<'HOST'
#!/bin/sh
printf 'root=%s\n' "$DEVCOUNCIL_ROOT"
printf '%s\n' "$@"
exit "${FAKE_HOST_EXIT:-0}"
HOST
chmod +x "$out"
`

type bootstrapRun struct {
	code   int
	out    string
	goArgs []string
}

func runBootstrap(t *testing.T, env []string, args ...string) bootstrapRun {
	t.Helper()
	if runtime.GOOS == "windows" {
		t.Skip("posix bootstrap")
	}
	root := repoRoot(t)
	bin := t.TempDir()
	if err := os.WriteFile(filepath.Join(bin, "go"), []byte(fakeGo), 0o755); err != nil {
		t.Fatal(err)
	}
	log := filepath.Join(t.TempDir(), "go.args")
	cmd := exec.Command("sh", append([]string{filepath.Join(root, "scripts", "install.sh")}, args...)...)
	cmd.Env = append(os.Environ(), "PATH="+bin+string(os.PathListSeparator)+os.Getenv("PATH"), "FAKE_GO_LOG="+log)
	cmd.Env = append(cmd.Env, env...)
	out, err := cmd.CombinedOutput()
	code := 0
	if exit, ok := err.(*exec.ExitError); ok {
		code = exit.ExitCode()
	} else if err != nil {
		t.Fatal(err)
	}
	logged, _ := os.ReadFile(log)
	return bootstrapRun{code: code, out: string(out), goArgs: strings.Fields(string(logged))}
}

func TestBootstrapBuildsTheHostFromThisCheckoutAndHandsItEveryArgument(t *testing.T) {
	root := repoRoot(t)
	r := runBootstrap(t, nil, "--only=devmap", "--prefix", "/x y", "--dry-run")
	if r.code != 0 {
		t.Fatalf("exit %d:\n%s", r.code, r.out)
	}
	want := "root=" + root + "\ninstall\n--only=devmap\n--prefix\n/x y\n--dry-run\n"
	if r.out != want {
		t.Fatalf("host saw:\n%s\nwant:\n%s", r.out, want)
	}
	gotGo := strings.Join(r.goArgs, " ")
	if !strings.HasPrefix(gotGo, "-C "+filepath.Join(root, "backend", "go_orchestrator")+" build -o ") ||
		!strings.HasSuffix(gotGo, " ./cmd/devcouncil") {
		t.Fatalf("go %s", gotGo)
	}
}

func TestBootstrapUninstallPicksTheVerbWhereverItAppears(t *testing.T) {
	for _, args := range [][]string{
		{"--uninstall", "dcgrep", "--yes"},
		{"dcgrep", "--uninstall", "--yes"},
	} {
		r := runBootstrap(t, nil, args...)
		if r.code != 0 || !strings.HasSuffix(r.out, "\nuninstall\ndcgrep\n--yes\n") {
			t.Fatalf("%v: exit %d, host saw:\n%s", args, r.code, r.out)
		}
	}
}

func TestBootstrapPassesHelpThroughToTheOneHelpText(t *testing.T) {
	r := runBootstrap(t, nil, "--help")
	if r.code != 0 || !strings.HasSuffix(r.out, "\ninstall\n--help\n") {
		t.Fatalf("exit %d, host saw:\n%s", r.code, r.out)
	}
}

func TestBootstrapExitsWithTheHostsStatus(t *testing.T) {
	r := runBootstrap(t, []string{"FAKE_HOST_EXIT=3"}, "uv")
	if r.code != 3 {
		t.Fatalf("exit %d, want the host's 3:\n%s", r.code, r.out)
	}
}

func TestBootstrapWithoutGoSaysSoAndNamesTheCargoRoute(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("posix bootstrap")
	}
	root := repoRoot(t)
	// An empty PATH: `command -v go` is a shell builtin and must fail before
	// the script needs any external tool.
	cmd := exec.Command("/bin/sh", filepath.Join(root, "scripts", "install.sh"), "devmap")
	cmd.Env = []string{"PATH=" + t.TempDir()}
	out, err := cmd.CombinedOutput()
	exit, ok := err.(*exec.ExitError)
	if !ok || exit.ExitCode() != 1 {
		t.Fatalf("err = %v:\n%s", err, out)
	}
	if !strings.Contains(string(out), "go is required") || !strings.Contains(string(out), "cargo install --git") {
		t.Fatalf("output:\n%s", out)
	}
}

func TestBootstrapLeavesNoTemporaryHostBehind(t *testing.T) {
	tmp := t.TempDir()
	r := runBootstrap(t, []string{"TMPDIR=" + tmp}, "host")
	if r.code != 0 {
		t.Fatalf("exit %d:\n%s", r.code, r.out)
	}
	entries, err := os.ReadDir(tmp)
	if err != nil {
		t.Fatal(err)
	}
	if len(entries) != 0 {
		t.Fatalf("bootstrap left %v in TMPDIR", entries)
	}
}

// DRY_RUN=1 was how the old scripts were asked to touch nothing. A bootstrap
// that dropped it would turn a rehearsal into a real install, so it becomes
// --dry-run; an empty or "0" value asks for nothing.
func TestBootstrapHonoursTheOldDryRunVariable(t *testing.T) {
	r := runBootstrap(t, []string{"DRY_RUN=1"}, "devmap")
	if r.code != 0 || !strings.HasSuffix(r.out, "\ninstall\ndevmap\n--dry-run\n") {
		t.Fatalf("exit %d, host saw:\n%s", r.code, r.out)
	}
	for _, off := range []string{"DRY_RUN=", "DRY_RUN=0"} {
		r := runBootstrap(t, []string{off}, "devmap")
		if r.code != 0 || strings.Contains(r.out, "--dry-run") {
			t.Fatalf("%s: exit %d, host saw:\n%s", off, r.code, r.out)
		}
	}
}
