package policy

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"testing"
)

// contracts/CHECKSUMS records the sha256 of every contract file. GitPulse and
// Manvi vendor the directory and check their copy against it, so a contract
// edited here without regenerating CHECKSUMS ships a manifest that describes a
// different contract. This test is the producer-side check; it replaced
// contracts/tools/checksums.py, which nothing in CI ran.
//
// The set of contract files is the directory, not a list: every regular,
// non-hidden file beside CHECKSUMS. Subdirectories (tools/) are machinery, not
// contract. A list kept in this file would let a new contract file go
// unrecorded, which is the drift the check exists to catch.

// expectedChecksums renders the CHECKSUMS body the contract files in dir
// require, in the established "<sha256>  <name>" form. Names keep the order
// recorded lists them in, new names follow sorted: the vendored consumers'
// own checkers compare the file byte for byte, so a pasted fix must not
// reorder lines that did not change.
func expectedChecksums(dir, recorded string) (string, error) {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return "", err
	}
	var names []string
	for _, entry := range entries {
		name := entry.Name()
		if !entry.Type().IsRegular() || name == "CHECKSUMS" || strings.HasPrefix(name, ".") {
			continue
		}
		names = append(names, name)
	}
	if len(names) == 0 {
		return "", fmt.Errorf("no contract files in %s", dir)
	}
	position := map[string]int{}
	for i, line := range strings.Split(recorded, "\n") {
		if _, name, ok := strings.Cut(line, "  "); ok {
			if _, seen := position[name]; !seen {
				position[name] = i
			}
		}
	}
	sort.Slice(names, func(i, j int) bool {
		pi, iRecorded := position[names[i]]
		pj, jRecorded := position[names[j]]
		if iRecorded != jRecorded {
			return iRecorded
		}
		if iRecorded {
			return pi < pj
		}
		return names[i] < names[j]
	})
	var body strings.Builder
	for _, name := range names {
		raw, err := os.ReadFile(filepath.Join(dir, name))
		if err != nil {
			return "", err
		}
		sum := sha256.Sum256(raw)
		fmt.Fprintf(&body, "%s  %s\n", hex.EncodeToString(sum[:]), name)
	}
	return body.String(), nil
}

// parseChecksums reads a CHECKSUMS body into name -> digest, refusing a line
// it cannot read rather than skipping it.
func parseChecksums(body string) (map[string]string, error) {
	out := map[string]string{}
	for i, line := range strings.Split(strings.TrimRight(body, "\n"), "\n") {
		digest, name, ok := strings.Cut(line, "  ")
		if !ok || len(digest) != sha256.Size*2 || name == "" {
			return nil, fmt.Errorf("CHECKSUMS line %d is not \"<sha256>  <name>\": %q", i+1, line)
		}
		if _, dup := out[name]; dup {
			return nil, fmt.Errorf("CHECKSUMS names %s twice", name)
		}
		out[name] = digest
	}
	return out, nil
}

// checksumDrift compares a recorded CHECKSUMS body with the one the files
// require and names every file that differs, is missing, or is unrecorded.
func checksumDrift(recorded, expected string) ([]string, error) {
	have, err := parseChecksums(recorded)
	if err != nil {
		return nil, err
	}
	want, err := parseChecksums(expected)
	if err != nil {
		return nil, err
	}
	var drift []string
	for name, digest := range want {
		switch got, ok := have[name]; {
		case !ok:
			drift = append(drift, fmt.Sprintf("%s: not recorded (actual %s)", name, digest))
		case got != digest:
			drift = append(drift, fmt.Sprintf("%s: recorded %s, actual %s", name, got, digest))
		}
	}
	for name := range have {
		if _, ok := want[name]; !ok {
			drift = append(drift, fmt.Sprintf("%s: recorded but no such contract file", name))
		}
	}
	sort.Strings(drift)
	return drift, nil
}

func TestContractChecksumsMatchTheContractFiles(t *testing.T) {
	dir := contractsDir(t)
	recorded, err := os.ReadFile(filepath.Join(dir, "CHECKSUMS"))
	if err != nil {
		t.Fatalf("reading CHECKSUMS: %v", err)
	}
	expected, err := expectedChecksums(dir, string(recorded))
	if err != nil {
		t.Fatalf("hashing contract files: %v", err)
	}
	drift, err := checksumDrift(string(recorded), expected)
	if err != nil {
		t.Fatal(err)
	}
	if len(drift) > 0 {
		t.Fatalf("contract files do not match %s:\n  %s\n\nIf the change was intended, "+
			"replace CHECKSUMS with:\n%s", filepath.Join(dir, "CHECKSUMS"),
			strings.Join(drift, "\n  "), expected)
	}
}

// The check above passes on a consistent tree, which on its own does not show
// it can fail. This drives the same comparison over a copy that drifted in each
// way a contract directory can.
func TestContractChecksumDriftIsDetected(t *testing.T) {
	dir := t.TempDir()
	write := func(name, body string) {
		t.Helper()
		if err := os.WriteFile(filepath.Join(dir, name), []byte(body), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	write("a.json", "{}\n")
	write("b.md", "# b\n")
	if err := os.Mkdir(filepath.Join(dir, "tools"), 0o755); err != nil {
		t.Fatal(err)
	}
	write("tools/gen.py", "machinery, not contract\n")
	write(".DS_Store", "finder noise\n")
	recorded, err := expectedChecksums(dir, "")
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(recorded, "gen.py") || strings.Contains(recorded, ".DS_Store") {
		t.Fatalf("subdirectories and hidden files are not contract files:\n%s", recorded)
	}
	if drift, err := checksumDrift(recorded, recorded); err != nil || len(drift) != 0 {
		t.Fatalf("a consistent directory reported drift %v (err %v)", drift, err)
	}

	cases := []struct {
		name   string
		mutate func()
		want   string
	}{
		{"edited", func() { write("a.json", "{\"x\":1}\n") }, "a.json: recorded"},
		{"added", func() { write("c.json", "[]\n") }, "c.json: not recorded"},
		{"removed", func() { os.Remove(filepath.Join(dir, "b.md")) }, "b.md: recorded but no such contract file"},
	}
	for _, c := range cases {
		c.mutate()
		expected, err := expectedChecksums(dir, recorded)
		if err != nil {
			t.Fatal(err)
		}
		drift, err := checksumDrift(recorded, expected)
		if err != nil {
			t.Fatal(err)
		}
		found := false
		for _, line := range drift {
			found = found || strings.HasPrefix(line, c.want)
		}
		if !found {
			t.Errorf("%s: want a drift line starting %q, got %v", c.name, c.want, drift)
		}
	}

	if _, err := checksumDrift("not a checksum line\n", recorded); err == nil {
		t.Error("an unreadable CHECKSUMS line must be refused, not skipped")
	}
}

// The replacement body a failure prints keeps the recorded order, so pasting
// it changes only the lines that drifted.
func TestExpectedChecksumsKeepTheRecordedOrder(t *testing.T) {
	dir := t.TempDir()
	for _, name := range []string{"a.md", "b.json", "c.json"} {
		if err := os.WriteFile(filepath.Join(dir, name), []byte(name), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	recorded := strings.Repeat("0", 64) + "  c.json\n" + strings.Repeat("0", 64) + "  a.md\n"
	body, err := expectedChecksums(dir, recorded)
	if err != nil {
		t.Fatal(err)
	}
	var order []string
	for _, line := range strings.Split(strings.TrimRight(body, "\n"), "\n") {
		_, name, _ := strings.Cut(line, "  ")
		order = append(order, name)
	}
	if got := strings.Join(order, ","); got != "c.json,a.md,b.json" {
		t.Fatalf("want recorded names first in recorded order, then new ones: got %s", got)
	}
}
