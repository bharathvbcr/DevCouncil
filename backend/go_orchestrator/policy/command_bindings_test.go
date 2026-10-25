package policy

import (
	"os"
	"path/filepath"
	"reflect"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
)

func TestExpandBindingsReadsWhatTheLineAndEnvironmentName(t *testing.T) {
	t.Setenv("HOME", "/home/agent")
	t.Setenv("TMPDIR", "/scratch")
	for _, tc := range []struct {
		line string
		want []string
	}{
		{"cat README.md", []string{"cat README.md"}},
		{"cat ~/x", []string{"cat /home/agent/x"}},
		{"cat ~", []string{"cat /home/agent"}},
		{"cat $HOME/x ${TMPDIR}/y", []string{"cat /home/agent/x /scratch/y"}},
		{"f=README.md; head -5 $f", []string{"f=README.md; head -5 README.md"}},
		{`export W=/tmp/w && cp go.work "$W/go.work"`, []string{`export W=/tmp/w && cp go.work "/tmp/w/go.work"`}},
		// sh expands a tilde after an assignment's `=` too.
		{"W=~/w; ls $W", []string{"W=/home/agent/w; ls /home/agent/w"}},
		{"for f in a.txt b.txt; do cat $f; done", []string{
			"for f in a.txt b.txt; do cat a.txt; done",
			"for f in a.txt b.txt; do cat b.txt; done",
		}},
		// Not expanded by sh, so not rewritten.
		{"echo '$HOME' \\$HOME", []string{"echo '$HOME' \\$HOME"}},
		{"echo a~b", []string{"echo a~b"}},
		{"cat ~other/x", []string{"cat ~other/x"}},
		// Unbound, special or computed: left as written.
		{"cat $OUT $1 $@ ${X:-y}", []string{"cat $OUT $1 $@ ${X:-y}"}},
		{"f=$(pick); cat $f", []string{"f=$(pick); cat $f"}},
		// An assignment prefixing a command binds nothing sh has not expanded.
		{"F=.env cat $F", []string{"F=.env cat $F"}},
		// A substitution is quoted afresh, so its tilde is live.
		{`echo "$(cat ~/x)"`, []string{`echo "$(cat /home/agent/x)"`}},
	} {
		if got := expandBindings(tc.line); !reflect.DeepEqual(got, tc.want) {
			t.Errorf("expandBindings(%q) =\n  %q\nwant\n  %q", tc.line, got, tc.want)
		}
	}
}

// A loop over many values is judged on maxBindingVariants readings, never on an
// unbounded number of them.
func TestExpandBindingsIsBounded(t *testing.T) {
	line := "for a in 1 2 3 4 5; do for b in 6 7 8 9 0; do echo $a$b; done; done"
	if got := expandBindings(line); len(got) != maxBindingVariants {
		t.Fatalf("got %d readings, want the cap %d", len(got), maxBindingVariants)
	}
}

func withTempDirs(t *testing.T, dirs ...string) {
	t.Helper()
	saved := tempDirs
	tempDirs = func() []string { return dirs }
	t.Cleanup(func() { tempDirs = saved })
}

// Writes into a temporary directory are allowed although they leave the root
// (owner decision, 2026-10-24); a symlink there is judged where it leads, and a
// root that itself lives in a temporary directory gets no allowance.
func TestTemporaryDirectoryWrites(t *testing.T) {
	root, scratch, elsewhere := t.TempDir(), t.TempDir(), t.TempDir()
	if err := os.Symlink(elsewhere, filepath.Join(scratch, "link")); err != nil {
		t.Fatal(err)
	}
	withTempDirs(t, scratch)
	gate := FileGate{Root: root, HardRules: true}

	for _, path := range []string{filepath.Join(scratch, "out.txt"), filepath.Join(scratch, "new", "dir", "f")} {
		if d := gate.EvaluateFileChange(path, nil, dc.OpWrite, false); d.Blocked() {
			t.Errorf("write to %s refused: %s %s", path, d.Rule, d.Reason)
		}
	}
	for _, path := range []string{
		filepath.Join(scratch, "link", "authorized_keys"),
		filepath.Join(elsewhere, "x"),
		filepath.Join(root, "..", "sibling"),
		scratch,
	} {
		if d := gate.EvaluateFileChange(path, nil, dc.OpWrite, false); !d.Blocked() || d.Rule != RuleOutsideRoot {
			t.Errorf("write to %s = %s/%s, want %s", path, d.Action, d.Rule, RuleOutsideRoot)
		}
	}

	// The same scratch path, from a root inside the temporary directory.
	inner := filepath.Join(scratch, "repo")
	if err := os.MkdirAll(inner, 0o755); err != nil {
		t.Fatal(err)
	}
	contained := FileGate{Root: inner, HardRules: true}
	if d := contained.EvaluateFileChange(filepath.Join(scratch, "out.txt"), nil, dc.OpWrite, false); d.Rule != RuleOutsideRoot {
		t.Errorf("a root under the temporary directory must keep its surroundings outside: %s/%s", d.Action, d.Rule)
	}

	// The command ladder agrees: an unmodelled program handed a scratch path
	// is not refused for it, and a redirect into it is a write the file rung
	// allows.
	commands := CommandGate{Root: root, HardRules: true}
	for _, command := range []string{"go build -o " + filepath.Join(scratch, "bin") + " ./...", "mkdir -p " + filepath.Join(scratch, "a", "b")} {
		if d := commands.EvaluateCommand(command, nil); d.Rule == RuleCommandOperandUnverifiable {
			t.Errorf("%q refused as %s: %s", command, d.Rule, d.Reason)
		}
	}
}
