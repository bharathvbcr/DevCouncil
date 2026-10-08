package integrate

import (
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// Host documents are written by `devmap integrate`, so these tests drive the
// real binary. HOME is a scratch directory for every one of them: DevMap also
// writes user-level configs (`~/.cursor/mcp.json`, `~/.claude.json`,
// `~/.codex/config.toml`), and a test must never reach the developer's own.

// fakeSelf stands in for this devcouncil binary in the entries written. It is
// never executed; it only has to be absolute and recognisable.
const fakeSelf = "/opt/devcouncil-test/bin/devcouncil"

// devmapForTest returns a devmap binary and isolates HOME.
func devmapForTest(t *testing.T) string {
	t.Helper()
	bin := testsupport.Devmap(t)
	t.Setenv("HOME", t.TempDir())
	return bin
}

// newRepo is a scratch repository whose DevMap state directory is already
// `.devcouncil/`. Without it, a host that writes under `.devcouncil/` (Warp)
// moves DevMap's state directory from `.devmap/` mid-run, and the next check
// sees the guides it wrote as drift — a DevMap behaviour, not this command's.
func newRepo(t *testing.T) string {
	t.Helper()
	root := t.TempDir()
	mustMkdirAll(t, filepath.Join(root, ".devcouncil"))
	return root
}

// devmapHosts is the host list read from the one place it lives: DevMap's own
// refusal of a name it does not take. Reading it there rather than writing it
// here is what keeps these tests from becoming a third copy of the list.
func devmapHosts(t *testing.T, bin string) []string {
	t.Helper()
	out, err := exec.Command(bin, "integrate", "not-a-host", "--dry-run").CombinedOutput()
	if err == nil {
		t.Fatalf("devmap accepted a bogus host:\n%s", out)
	}
	m := regexp.MustCompile(`possible values: ([a-z, -]+)`).FindSubmatch(out)
	if m == nil {
		t.Fatalf("devmap's refusal names no hosts:\n%s", out)
	}
	hosts := strings.Split(string(m[1]), ", ")
	if len(hosts) < 2 {
		t.Fatalf("read %v as devmap's hosts from:\n%s", hosts, out)
	}
	return hosts
}

func apply(t *testing.T, bin, root, host string) *Receipt {
	t.Helper()
	receipt, err := Run(Options{Root: root, Host: host, Mode: ModeApply, DevmapBin: bin, SelfBin: fakeSelf})
	if err != nil {
		t.Fatalf("integrate %s --apply: %v", host, err)
	}
	return receipt
}

// Every host DevMap takes gets DevCouncil's server registered, and a check
// straight after agrees the installation is current.
func TestEveryHostRegistersDevCouncilAndAChecksAgrees(t *testing.T) {
	bin := devmapForTest(t)
	for _, host := range devmapHosts(t, bin) {
		t.Run(host, func(t *testing.T) {
			t.Setenv("HOME", t.TempDir())
			root := newRepo(t)
			receipt := apply(t, bin, root, host)
			if !registersDevCouncil(t, root, receipt) {
				t.Fatalf("%s: no written file carries the devcouncil entry: %v", host, receipt.Files)
			}
			check, err := Run(Options{Root: root, Host: host, Mode: ModeCheck, DevmapBin: bin, SelfBin: fakeSelf})
			if err != nil {
				t.Fatal(err)
			}
			for path, action := range check.Files {
				if action != "unchanged" {
					t.Errorf("%s: check reports %s as %q straight after an apply", host, path, action)
				}
			}
		})
	}
}

// registersDevCouncil reports whether some file the receipt names carries
// DevCouncil's entry: this binary, scoped to this repository.
func registersDevCouncil(t *testing.T, root string, receipt *Receipt) bool {
	t.Helper()
	for path := range receipt.Files {
		if !filepath.IsAbs(path) {
			path = filepath.Join(root, path)
		}
		body, err := os.ReadFile(path)
		if err != nil {
			continue
		}
		text := string(body)
		if strings.Contains(text, fakeSelf) && strings.Contains(text, "DEVCOUNCIL_PROJECT_ROOT") {
			return true
		}
	}
	return false
}

// A dry run and a check write nothing into the repository.
func TestDryRunAndCheckWriteNothing(t *testing.T) {
	bin := devmapForTest(t)
	for _, mode := range []Mode{ModeDryRun, ModeCheck} {
		t.Run(string(mode), func(t *testing.T) {
			root := t.TempDir()
			receipt, err := Run(Options{Root: root, Host: "cursor", Mode: mode, DevmapBin: bin, SelfBin: fakeSelf})
			if err != nil {
				t.Fatal(err)
			}
			if len(receipt.Files) == 0 {
				t.Fatal("a read-only run reported no files at all")
			}
			if entries, _ := os.ReadDir(root); len(entries) != 0 {
				t.Fatalf("%s wrote into the repository: %v", mode, entries)
			}
		})
	}
}

// Without a devmap to run, nothing can be configured, and saying so is the
// only honest receipt. The old path returned success with a note.
func TestNoDevmapIsARefusalNotASuccess(t *testing.T) {
	t.Setenv("DEVMAP_BIN", "")
	t.Setenv("PATH", t.TempDir())
	root := t.TempDir()
	receipt, err := Run(Options{Root: root, Host: "cursor", Mode: ModeApply, SelfBin: fakeSelf})
	if err == nil || !strings.Contains(err.Error(), "nothing was configured") {
		t.Fatalf("want a refusal naming what did not happen, got %v (receipt %+v)", err, receipt)
	}
	if entries, _ := os.ReadDir(root); len(entries) != 0 {
		t.Fatalf("wrote without devmap: %v", entries)
	}
	if _, err := Run(Options{Root: root, Host: "cursor", Mode: ModeApply, SelfBin: fakeSelf,
		DevmapBin: filepath.Join(root, "missing-devmap")}); err == nil {
		t.Fatal("an explicit devmap that does not exist was replaced instead of refused")
	}
}

func mustJSON(v any) []byte {
	b, err := json.MarshalIndent(v, "", "  ")
	if err != nil {
		panic(err)
	}
	return append(b, '\n')
}
