package policy

import (
	"path/filepath"
	"testing"
)

// A tilde resolves to the home directory in the spelling the shell reads. On
// Windows the home directory is "C:\Users\x"; a backslash is a shell escape, so
// the binding was refused outright and "~/.ssh/k" stayed the relative path it
// spells, judged inside the repository while the shell writes it under the home
// directory.
func TestATildeResolvesToTheHomeDirectoryOnEveryPlatform(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)        // read by os.UserHomeDir off Windows
	t.Setenv("USERPROFILE", home) // and on it
	want := filepath.ToSlash(home) + "/.ssh/k"
	got := expandBindings("echo x > ~/.ssh/k")
	if len(got) != 1 || got[0] != "echo x > "+want {
		t.Fatalf("expandBindings = %q, want [%q]", got, "echo x > "+want)
	}
}
