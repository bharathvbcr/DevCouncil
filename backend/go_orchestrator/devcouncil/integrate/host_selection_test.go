package integrate

import (
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
)

// A host this command cannot configure must be refused before anything is
// written or spawned. Until `checkHost` existed, an unknown name fell through
// to a note on an otherwise ordinary receipt: `integrate banana --apply`
// exited 0, reported success, and spawned `devmap integrate banana` first.
func TestUnknownHostIsRefusedAndWritesNothing(t *testing.T) {
	for _, host := range []string{"banana", "vscode", "GEMINI-2", "  "} {
		t.Run(host, func(t *testing.T) {
			root := t.TempDir()
			receipt, err := Run(Options{Root: root, Host: host, Mode: ModeApply})
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
// invalid. Someone typing `integrate gemini` has run it before.
func TestRetiredHostsExplainTheSuccessor(t *testing.T) {
	// Each retired name must be answered with the thing to do instead, not
	// merely that it is unknown.
	for host, want := range map[string]string{
		"gemini": "antigravity",
		"aider":  "no mcp server",
	} {
		t.Run(host, func(t *testing.T) {
			_, err := Run(Options{Root: t.TempDir(), Host: host, Mode: ModeApply})
			if err == nil {
				t.Fatalf("%s was accepted", host)
			}
			if !strings.Contains(strings.ToLower(err.Error()), want) {
				t.Fatalf("refusal does not point at %q: %v", want, err)
			}
		})
	}
}

func TestRetiredHostsAreNotOffered(t *testing.T) {
	for _, host := range []string{"gemini", "aider"} {
		if slices.Contains(Hosts, host) {
			t.Fatalf("%s is still advertised in Hosts", host)
		}
		if _, known := retiredHosts[host]; !known {
			t.Fatalf("%s was dropped without an explanation for callers", host)
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

// The two lists that disagreed are the reason a name could be accepted here
// and refused by the binary this command then spawns.
func TestAdvertisedHostsMatchTheRustIntegrator(t *testing.T) {
	// Mirrors `Host::parse` in rust/devmap-cli/src/integrate.rs.
	rust := []string{"cursor", "claude", "codex", "antigravity", "opencode", "warp"}
	got := slices.Clone(Hosts)
	slices.Sort(got)
	slices.Sort(rust)
	if !slices.Equal(got, rust) {
		t.Fatalf("host lists have drifted:\n  go:   %v\n  rust: %v", got, rust)
	}
}

func TestSupportedHostsAreStillAccepted(t *testing.T) {
	for _, host := range Hosts {
		if err := checkHost(host); err != nil {
			t.Fatalf("%s is advertised but refused: %v", host, err)
		}
	}
}
