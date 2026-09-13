package integrate

import (
	"encoding/json"
	"os"
	"path/filepath"
	"sort"
	"testing"
)

func readJSON(t *testing.T, path string) map[string]any {
	t.Helper()
	b, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var out map[string]any
	if err := json.Unmarshal(b, &out); err != nil {
		t.Fatalf("%s is not JSON: %v\n%s", path, err, b)
	}
	return out
}

// servers returns the host's server map, whatever it is nested under.
func servers(t *testing.T, doc hostMcpDoc, root string) map[string]any {
	t.Helper()
	file := readJSON(t, filepath.Join(root, doc.rel))
	if doc.container == "" {
		return file
	}
	inner, ok := file[doc.container].(map[string]any)
	if !ok {
		t.Fatalf("%s: %q is not an object", doc.rel, doc.container)
	}
	return inner
}

func apply(t *testing.T, root, host string) *Receipt {
	t.Helper()
	receipt, err := Run(Options{Root: root, Host: host, Mode: ModeApply})
	if err != nil {
		t.Fatalf("integrate %s: %v", host, err)
	}
	return receipt
}

// The server list a host already had is not ours to edit. Only our own entry
// is added or replaced.
func TestEachHostRegistersWithoutDisturbingNeighbours(t *testing.T) {
	for host, doc := range hostMcpDocs {
		t.Run(host, func(t *testing.T) {
			root := t.TempDir()
			path := filepath.Join(root, doc.rel)
			if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
				t.Fatal(err)
			}
			existing := map[string]any{"theirs": map[string]any{"command": "other"}}
			if doc.container != "" {
				existing = map[string]any{doc.container: existing, "unrelated": "keep me"}
			}
			b, _ := json.Marshal(existing)
			if err := os.WriteFile(path, b, 0o644); err != nil {
				t.Fatal(err)
			}

			apply(t, root, host)
			got := servers(t, doc, root)
			if _, ours := got[mcpServerName]; !ours {
				t.Fatalf("%s: our server was not registered: %v", host, got)
			}
			if _, theirs := got["theirs"]; !theirs {
				t.Fatalf("%s: dropped an unrelated server: %v", host, got)
			}
			if doc.container != "" {
				if readJSON(t, path)["unrelated"] != "keep me" {
					t.Fatalf("%s: dropped an unrelated top-level key", host)
				}
			}
		})
	}
}

// OpenCode's entry names the program as one argv array; the other two use a
// command plus a separate args list. Getting this wrong yields a config the
// host silently ignores.
func TestEntryShapeMatchesTheHost(t *testing.T) {
	for host, doc := range hostMcpDocs {
		t.Run(host, func(t *testing.T) {
			root := t.TempDir()
			apply(t, root, host)
			entry, ok := servers(t, doc, root)[mcpServerName].(map[string]any)
			if !ok {
				t.Fatalf("%s: entry is not an object", host)
			}
			_, argv := entry["command"].([]any)
			if argv != doc.argvForm {
				t.Fatalf("%s: argv form is %v, want %v (entry %v)", host, argv, doc.argvForm, entry)
			}
			if doc.argvForm {
				if entry["type"] != "local" {
					t.Fatalf("%s: opencode entries declare a local type: %v", host, entry)
				}
			} else if _, hasArgs := entry["args"].([]any); !hasArgs {
				t.Fatalf("%s: expected a separate args list: %v", host, entry)
			}
		})
	}
}

// A preamble key establishes a default; it does not overrule a user who pinned
// something else on purpose.
func TestPreambleIsEstablishedNotImposed(t *testing.T) {
	doc := hostMcpDocs["opencode"]
	root := t.TempDir()
	path := filepath.Join(root, doc.rel)
	if err := os.WriteFile(path, []byte(`{"$schema":"https://opencode.ai/config-v2.json"}`), 0o644); err != nil {
		t.Fatal(err)
	}
	apply(t, root, "opencode")
	if got := readJSON(t, path)["$schema"]; got != "https://opencode.ai/config-v2.json" {
		t.Fatalf("overwrote a pinned schema: %v", got)
	}

	fresh := t.TempDir()
	apply(t, fresh, "opencode")
	if got := readJSON(t, filepath.Join(fresh, doc.rel))["$schema"]; got != "https://opencode.ai/config.json" {
		t.Fatalf("a new file did not get the default schema: %v", got)
	}
}

// Applying twice must leave the file alone the second time, and `--check` must
// then agree it is current. Before `planWrite` compared against the *merged*
// result, a file holding our entry beside someone else's always read as drift.
func TestASecondApplyIsCleanAndCheckAgrees(t *testing.T) {
	for host, doc := range hostMcpDocs {
		t.Run(host, func(t *testing.T) {
			root := t.TempDir()
			apply(t, root, host)
			path := filepath.Join(root, doc.rel)
			first, err := os.ReadFile(path)
			if err != nil {
				t.Fatal(err)
			}
			apply(t, root, host)
			second, err := os.ReadFile(path)
			if err != nil {
				t.Fatal(err)
			}
			if string(first) != string(second) {
				t.Fatalf("%s: a second apply changed bytes", host)
			}
			receipt, err := Run(Options{Root: root, Host: host, Mode: ModeCheck})
			if err != nil {
				t.Fatal(err)
			}
			if got := receipt.Files[doc.rel]; got != "unchanged" {
				t.Fatalf("%s: check reports %q for an up-to-date file", host, got)
			}
		})
	}
}

// A config this command cannot read is kept, not replaced.
func TestUnreadableConfigIsRefusedNotOverwritten(t *testing.T) {
	cases := map[string]string{
		"not-json":        `{ this is not json`,
		"wrong-container": `{"mcp": "not an object"}`,
		"has-comments":    "{\n  // a comment encoding/json cannot round-trip\n  \"mcp\": {}\n}",
	}
	doc := hostMcpDocs["opencode"]
	for label, body := range cases {
		t.Run(label, func(t *testing.T) {
			root := t.TempDir()
			path := filepath.Join(root, doc.rel)
			if err := os.WriteFile(path, []byte(body), 0o644); err != nil {
				t.Fatal(err)
			}
			if _, err := Run(Options{Root: root, Host: "opencode", Mode: ModeApply}); err == nil {
				t.Fatalf("%s was accepted", label)
			}
			after, err := os.ReadFile(path)
			if err != nil {
				t.Fatal(err)
			}
			if string(after) != body {
				t.Fatalf("%s was rewritten despite being refused:\n%s", label, after)
			}
		})
	}
}

func TestDryRunWritesNothing(t *testing.T) {
	for host, doc := range hostMcpDocs {
		t.Run(host, func(t *testing.T) {
			root := t.TempDir()
			if _, err := Run(Options{Root: root, Host: host, Mode: ModeDryRun}); err != nil {
				t.Fatal(err)
			}
			if _, err := os.Stat(filepath.Join(root, doc.rel)); !os.IsNotExist(err) {
				t.Fatalf("%s: dry run created %s", host, doc.rel)
			}
		})
	}
}

// The Rust integrator writes the `devmap` entry into these same files. Two
// tables that disagree about where a host keeps its servers would have the two
// binaries writing to different places, or to different keys in one place.
func TestHostDocumentsMatchTheRustIntegrator(t *testing.T) {
	// Mirrors `Host::mcp_document` in rust/devmap-cli/src/integrate.rs.
	rust := map[string]struct{ rel, container string }{
		"antigravity": {".agents/mcp_config.json", "mcpServers"},
		"opencode":    {"opencode.json", "mcp"},
		"warp":        {".devcouncil/integrations/warp-mcp.json", ""},
	}
	var names []string
	for host := range hostMcpDocs {
		names = append(names, host)
	}
	sort.Strings(names)
	if len(names) != len(rust) {
		t.Fatalf("host document tables have drifted: go has %v", names)
	}
	for _, host := range names {
		want, known := rust[host]
		if !known {
			t.Fatalf("%s has no counterpart in the Rust table", host)
		}
		got := hostMcpDocs[host]
		if got.rel != want.rel || got.container != want.container {
			t.Fatalf("%s: go writes %q/%q, rust writes %q/%q",
				host, got.rel, got.container, want.rel, want.container)
		}
	}
}
