//go:build windows

package dc

import (
	"strings"
	"testing"
)

// writeKeyWithSDDL creates a key file whose DACL is the one sddl describes.
func writeKeyWithSDDL(t *testing.T, path string, data []byte, sddl string) {
	t.Helper()
	f, err := createFileWithSDDL(path, sddl)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := f.Write(data); err != nil {
		t.Fatal(err)
	}
	if err := f.Close(); err != nil {
		t.Fatal(err)
	}
}

// writeLooseKeyFile writes a key file whose DACL admits Users, which is what a
// file inherits in a directory the whole machine can read.
func writeLooseKeyFile(t *testing.T, path string, data []byte) {
	t.Helper()
	writeKeyWithSDDL(t, path, data, "D:P(A;;FA;;;OW)(A;;FR;;;BU)")
}

func TestWindowsKeyDACLDecisions(t *testing.T) {
	keyFile, _, err := GenerateSigningKey("k")
	if err != nil {
		t.Fatal(err)
	}
	cases := []struct {
		name    string
		sddl    string
		refused string // substring of the refusal, empty when the key must load
	}{
		{"owner only", ownerOnlySDDL, ""},
		{"owner, SYSTEM and Administrators", "D:P(A;;FA;;;OW)(A;;FA;;;SY)(A;;FA;;;BA)", ""},
		// The deny names Anonymous Logon, which no owner's token holds. A deny
		// to Users (the first version) also denies the owner, who is a member,
		// and FR includes READ_CONTROL, so the descriptor itself could not be
		// read and the load correctly failed closed.
		{"a deny entry only narrows access", "D:P(D;;FR;;;AN)(A;;FA;;;OW)", ""},
		{"Users", "D:P(A;;FA;;;OW)(A;;FR;;;BU)", "S-1-5-32-545"},
		{"Authenticated Users", "D:P(A;;FA;;;OW)(A;;FR;;;AU)", "S-1-5-11"},
		{"Everyone", "D:P(A;;FA;;;OW)(A;;FR;;;WD)", "S-1-1-0"},
		{"another account", "D:P(A;;FA;;;OW)(A;;FR;;;LG)", "not its owner"}, // Guests
		{"no DACL at all", "D:NO_ACCESS_CONTROL", "no DACL"},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			path := t.TempDir() + `\k.json`
			writeKeyWithSDDL(t, path, keyFile, c.sddl)
			_, _, err := LoadSigningKey(path)
			if c.refused == "" {
				if err != nil {
					t.Fatalf("an owner-only key was refused: %v", err)
				}
				return
			}
			if err == nil {
				t.Fatalf("a key with DACL %q was accepted", c.sddl)
			}
			if !strings.Contains(err.Error(), c.refused) {
				t.Fatalf("refusal %q does not name %q", err, c.refused)
			}
		})
	}
}

// The file keygen creates is accepted, and CreateSigningKeyFile never
// overwrites an existing key.
func TestWindowsCreateSigningKeyFileIsOwnerOnlyAndExclusive(t *testing.T) {
	path := t.TempDir() + `\k.json`
	f, err := CreateSigningKeyFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if err := f.Close(); err != nil {
		t.Fatal(err)
	}
	if err := checkKeyFileAccess(path, nil); err != nil {
		t.Fatalf("a freshly created key file was refused: %v", err)
	}
	if g, err := CreateSigningKeyFile(path); err == nil {
		g.Close()
		t.Fatal("CreateSigningKeyFile overwrote an existing key file")
	}
}
