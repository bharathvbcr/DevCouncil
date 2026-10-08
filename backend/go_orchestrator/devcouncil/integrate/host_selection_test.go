package integrate

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

// A host this command cannot configure is refused before anything is written,
// and without a receipt. `devmap integrate` owns the host list and refuses an
// unknown name before it opens a file; this command must pass that refusal on
// rather than wrap it in a receipt that reads as a result. Until a refusal
// existed, `integrate banana --apply` exited 0 and reported success.
func TestUnknownHostIsRefusedAndWritesNothing(t *testing.T) {
	bin := devmapForTest(t)
	for _, host := range []string{"banana", "vscode", "GEMINI-2", "  "} {
		t.Run(host, func(t *testing.T) {
			root := t.TempDir()
			receipt, err := Run(Options{Root: root, Host: host, Mode: ModeApply, DevmapBin: bin, SelfBin: fakeSelf})
			if err == nil {
				t.Fatalf("accepted unsupported host %q (receipt %+v)", host, receipt)
			}
			if receipt != nil {
				t.Fatalf("returned a receipt for a refused host: %+v", receipt)
			}
			entries, readErr := os.ReadDir(root)
			if readErr != nil {
				t.Fatal(readErr)
			}
			if len(entries) != 0 {
				t.Fatalf("refused host still wrote %d entr(ies)", len(entries))
			}
		})
	}
}

// A name that used to work is answered with where it went, not just that it is
// invalid. Someone typing `integrate gemini` has run it before. Answered here,
// before any devmap is needed: the explanation is DevCouncil's.
func TestRetiredHostsExplainTheSuccessor(t *testing.T) {
	for host, want := range map[string]string{
		"gemini": "antigravity",
		"aider":  "no mcp server",
	} {
		t.Run(host, func(t *testing.T) {
			_, err := Run(Options{Root: t.TempDir(), Host: host, Mode: ModeApply,
				DevmapBin: filepath.Join(t.TempDir(), "never-run")})
			if err == nil {
				t.Fatalf("%s was accepted", host)
			}
			if !strings.Contains(strings.ToLower(err.Error()), want) {
				t.Fatalf("refusal does not point at %q: %v", want, err)
			}
		})
	}
}

// A retired name must not also be one DevMap configures. Retirement is checked
// first, so a name in both places would be refused here while the integrator
// that owns the host list supports it — the drift of two lists, one level up.
func TestRetiredHostsAreNotHostsDevmapTakes(t *testing.T) {
	bin := devmapForTest(t)
	for _, offered := range devmapHosts(t, bin) {
		if _, retired := retiredHosts[offered]; retired {
			t.Errorf("%s is both retired here and configured by devmap integrate", offered)
		}
	}
	for host := range retiredHosts {
		if out, err := exec.Command(bin, "integrate", host, "--dry-run").CombinedOutput(); err == nil {
			t.Errorf("devmap integrate accepts retired host %s:\n%s", host, out)
		}
	}
}

// Dropping a host from installation says nothing about removal. A
// `.gemini/settings.json` already on disk must stay cleanable by the tool that
// no longer writes it, or retiring an adapter strands every registration it
// ever made.
func TestRetiringAHostKeepsItsCleanupPath(t *testing.T) {
	root := t.TempDir()
	path := filepath.Join(root, ".gemini", "settings.json")
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	const before = `{"theme":"dark","hooks":{"BeforeTool":[{"hooks":[{"command":"dev hook pre-tool-use"},{"command":"keep-me"}]}]}}`
	if err := os.WriteFile(path, []byte(before), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := Uninstall(UninstallOptions{Root: root, Client: "gemini", Mode: ModeApply}); err != nil {
		t.Fatalf("cleanup refused a retired host: %v", err)
	}
	after, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	text := string(after)
	if strings.Contains(text, "dev hook") {
		t.Fatalf("registration survived cleanup: %s", text)
	}
	if !strings.Contains(text, "keep-me") || !strings.Contains(text, "dark") {
		t.Fatalf("cleanup damaged unrelated settings: %s", text)
	}
}
