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
		if got, truncated := expandBindings(tc.line); truncated || !reflect.DeepEqual(got, tc.want) {
			t.Errorf("expandBindings(%q) =\n  %q\nwant\n  %q", tc.line, got, tc.want)
		}
	}
}

// A loop over many values is judged on at most maxBindingVariants readings,
// never an unbounded number of them, and a line past the cap says so instead of
// quietly dropping the readings it did not render.
func TestExpandBindingsIsBounded(t *testing.T) {
	line := "for a in 1 2 3 4 5; do for b in 6 7 8 9 0; do echo $a$b; done; done"
	got, truncated := expandBindings(line)
	if len(got) != maxBindingVariants || !truncated {
		t.Fatalf("got %d readings truncated=%v, want the cap %d and truncated", len(got), truncated, maxBindingVariants)
	}
}

// A name that would take a value past maxBindingValues is reported, but only
// when the line references it: an unused long loop list is not a dropped reading.
func TestExpandBindingsReportsDroppedValues(t *testing.T) {
	long := "for f in a b c d e f g h i j k l m n o p .env; do "
	if _, truncated := expandBindings(long + "cat $f; done"); !truncated {
		t.Fatal("a referenced name that lost a value must report truncation")
	}
	if _, truncated := expandBindings(long + "echo hi; done"); truncated {
		t.Fatal("an unreferenced name must not report truncation")
	}
}

// The for-header parser binds every word after `in`, reserved words included.
func TestForHeaderBindsEveryItem(t *testing.T) {
	got, truncated := expandBindings("for f in a do .env; do cat $f; done")
	if truncated || len(got) != 3 || got[2] != "for f in a do .env; do cat .env; done" {
		t.Fatalf("got %q truncated=%v, want a reading for each of a, do, .env", got, truncated)
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

// unboundOperand names the expansions the gate cannot resolve (ft-ee1b04e7) and
// leaves literals, quoted text and revision syntax alone.
func TestUnboundOperand(t *testing.T) {
	for word, want := range map[string]bool{
		"$zz": true, "${zz:-.env}": true, "${f/a/.env}": true, "${f%a}.env": true, "${!v}": true,
		"$1": true, "$@": true, "--file=$zz": true, "{.env,x}": true, "x{,.env}": true, "{a..c}": true,
		"'$zz'": false, "'{.env,x}'": false, "a$": false, "README.md": false, "{}": false,
		"HEAD@{1}": false, "HEAD@{2.days.ago}": false, "*.go": false, "a\\$b": false,
	} {
		words := shellWords("cat " + word)
		if len(words) != 2 {
			t.Fatalf("%q read as %d words", word, len(words))
		}
		if got := unboundOperand(words[1]); got != want {
			t.Errorf("unboundOperand(%q) = %v, want %v", word, got, want)
		}
	}
}

// computedOperand keeps the owner's judge-by-spelling decision for a bare
// unbound variable and refuses the computed forms.
func TestComputedOperandSparesBareUnboundVariables(t *testing.T) {
	for word, want := range map[string]bool{
		"$b": false, "${b}": false, "main..$b": false, "$c:README.md": false, "$OUT/x": false,
		"${b:-x}": true, "${b/a/b}": true, "$1": true, "$@": true, "{a,b}": true, "${#b}": true,
	} {
		words := shellWords("cat " + word)
		if got := computedOperand(words[1]); got != want {
			t.Errorf("computedOperand(%q) = %v, want %v", word, got, want)
		}
	}
}

func TestComputedOperandSpecialsAndANSIC(t *testing.T) {
	for word, want := range map[string]bool{
		"$_": true, "${_}": true, "$0": true, "$$": true, "$-": true, "$!": true, "$?": true,
		"$_x": false, "$__": false, "${_x}": false,
		"$'.env'": true, "$'\\x2eenv'": true, "HEAD:$'.env'": true, "'a$'": false,
	} {
		words := shellWords("cat " + word)
		if got := computedOperand(words[1]); got != want {
			t.Errorf("computedOperand(%q) = %v, want %v", word, got, want)
		}
	}
}
