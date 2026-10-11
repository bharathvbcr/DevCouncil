package integrate

import (
	"bytes"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// Every temp HOME here is the test's own: Run and Remove drive a real devmap
// that writes user-level host documents under $HOME, so nothing in this file
// may run against the real ~/.codex, ~/.cursor or ~/.claude.json.

func removeOpts(root, host string, mode Mode, bin string) RemoveOptions {
	return RemoveOptions{Root: root, Host: host, Mode: mode, DevmapBin: bin}
}

// Apply then remove puts every host back: DevCouncil's and DevMap's entries
// are gone, and the user's own documents are what they were.
func TestRemoveUndoesAnApplyAndLeavesTheUsersDocumentsAlone(t *testing.T) {
	bin := devmapForTest(t)
	for _, host := range devmapHosts(t, bin) {
		t.Run(host, func(t *testing.T) {
			home := t.TempDir()
			t.Setenv("HOME", home)
			root := newRepo(t)

			userDocs := map[string][]byte{}
			seed := func(path string, body []byte) {
				mustMkdirAll(t, filepath.Dir(path))
				if err := os.WriteFile(path, body, 0o644); err != nil {
					t.Fatal(err)
				}
				userDocs[path] = body
			}
			switch host {
			case "codex":
				seed(filepath.Join(home, ".codex", "config.toml"),
					[]byte("# my codex config\nmodel = \"o5\"   # trailing comment\n\n[mcp_servers.other]\ncommand = \"x\"   # theirs\nargs = []\n"))
			case "cursor":
				seed(filepath.Join(home, ".cursor", "mcp.json"),
					mustJSON(map[string]any{"mcpServers": map[string]any{"other": map[string]any{"command": "x"}}}))
			}

			receipt := apply(t, bin, root, host)
			if !registersDevCouncil(t, root, receipt) {
				t.Fatalf("apply registered nothing to remove: %v", receipt.Files)
			}

			// Check first: it reports what remains and changes nothing.
			before, err := Remove(removeOpts(root, host, ModeCheck, bin))
			if err != nil {
				t.Fatal(err)
			}
			if len(before.Present()) == 0 {
				t.Fatalf("a check straight after an apply found nothing registered: %+v", before)
			}
			if !registersDevCouncil(t, root, receipt) {
				t.Fatal("--check removed something")
			}

			removed, err := Remove(removeOpts(root, host, ModeApply, bin))
			if err != nil {
				t.Fatal(err)
			}
			if registersDevCouncil(t, root, receipt) {
				t.Fatalf("devcouncil is still registered after a remove: %+v", removed)
			}
			after, err := Remove(removeOpts(root, host, ModeCheck, bin))
			if !errors.Is(err, ErrNoReceipt) {
				t.Fatalf("a second remove has no receipt left to follow: err=%v receipt=%+v", err, after)
			}
			for path, original := range userDocs {
				got, err := os.ReadFile(path)
				if err != nil {
					t.Fatal(err)
				}
				switch {
				case strings.HasSuffix(path, ".toml"):
					if !bytes.Equal(got, original) {
						t.Fatalf("%s changed:\n--- before\n%s--- after\n%s", path, original, got)
					}
				default:
					var a, b any
					_ = json.Unmarshal(original, &a)
					if json.Unmarshal(got, &b) != nil || !jsonEqual(a, b) {
						t.Fatalf("%s changed: %s", path, got)
					}
				}
			}
		})
	}
}

func jsonEqual(a, b any) bool {
	x, _ := json.Marshal(a)
	y, _ := json.Marshal(b)
	return bytes.Equal(x, y)
}

// An undo with no receipt would have to guess what to take back, and it does
// not: it refuses, and touches nothing.
func TestRemoveWithoutAReceiptRefuses(t *testing.T) {
	bin := devmapForTest(t)
	root := newRepo(t)
	path := filepath.Join(root, ".mcp.json")
	body := []byte(`{"mcpServers":{"devcouncil":{"command":"/mine"}}}` + "\n")
	if err := os.WriteFile(path, body, 0o644); err != nil {
		t.Fatal(err)
	}
	_, err := Remove(removeOpts(root, "claude", ModeApply, bin))
	if !errors.Is(err, ErrNoReceipt) {
		t.Fatalf("err = %v, want ErrNoReceipt", err)
	}
	if got, _ := os.ReadFile(path); !bytes.Equal(got, body) {
		t.Fatalf("a refused remove changed the document: %s", got)
	}
}

// The Cursor rule is DevCouncil's own file. A remove takes it, and keeps a
// copy of any lines the user added to it.
func TestRemoveTakesTheCursorRuleAndBacksUpEdits(t *testing.T) {
	bin := devmapForTest(t)
	root := newRepo(t)
	apply(t, bin, root, "cursor")
	rule := filepath.Join(root, ".cursor", "rules", "devcouncil.mdc")
	edited := append(mustRead(t, rule), []byte("\nMy own addition.\n")...)
	if err := os.WriteFile(rule, edited, 0o644); err != nil {
		t.Fatal(err)
	}
	receipt, err := Remove(removeOpts(root, "cursor", ModeApply, bin))
	if err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(rule); !os.IsNotExist(err) {
		t.Fatalf("the rule is still there (stat err %v)", err)
	}
	backup := receipt.Backups[".cursor/rules/devcouncil.mdc"]
	if backup == "" || !bytes.Equal(mustRead(t, filepath.Join(root, backup)), edited) {
		t.Fatalf("the edited rule was not preserved: %+v", receipt)
	}
}

func mustRead(t *testing.T, path string) []byte {
	t.Helper()
	b, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	return b
}

// The receipt is the record of an apply and is only extended by later ones: an
// apply that finds everything current does not erase what the first one wrote.
func TestApplyAccumulatesItsReceipt(t *testing.T) {
	bin := devmapForTest(t)
	root := newRepo(t)
	first := apply(t, bin, root, "claude")
	second := apply(t, bin, root, "claude")
	for path, state := range second.Files {
		if state != "unchanged" && state != "not_ours" {
			t.Fatalf("second apply changed %s: %s", path, state)
		}
	}
	recorded := mustRead(t, filepath.Join(root, receiptRel("claude")))
	for path, state := range first.Files {
		if state == "wrote" && !bytes.Contains(recorded, []byte(path)) {
			t.Fatalf("receipt lost %s after a no-op apply:\n%s", path, recorded)
		}
	}
}
