package policy

import (
	"fmt"
	"strings"
	"testing"
)

// RedirectTargets used to know only output redirections, so the write rung
// above this package judged `echo x > docs/other.md` and waved through
// `sed -i s/a/b/ docs/other.md`, `tee docs/other.md`, `cp a docs/other.md` and
// `mv a docs/other.md` — the same write, spelled as an argument instead of an
// operator. Under a host posture that demotes command.not_allowed, those four
// were allowed outright while the redirect was refused as scope.unplanned.
//
// These cases pin the files each command writes through its arguments. Order
// is the order the function reports them in: this clause's redirections and
// substitutions first, then its argument writes.
func TestArgumentWriteTargets(t *testing.T) {
	tests := []struct {
		input   string
		targets []string
		opaque  bool
	}{
		// sed: only an in-place edit writes, and every spelling of it counts.
		{"sed -i s/a/b/ docs/other.md", []string{"docs/other.md"}, false},
		{"sed -i.bak s/a/b/ f.txt", []string{"f.txt", "f.txt.bak"}, false},
		{"sed -i'' s/a/b/ f.txt", []string{"f.txt"}, false},
		{"sed -i '' s/a/b/ f.txt", []string{"f.txt"}, false}, // macOS form
		{"sed -i .bak s/a/b/ d/f.txt", []string{"d/f.txt", "d/f.txt.bak"}, false},
		{"sed --in-place s/a/b/ f.txt", []string{"f.txt"}, false},
		{"sed --in-place=.orig s/a/b/ f.txt", []string{"f.txt", "f.txt.orig"}, false},
		{"sed -i'bak/*' s/a/b/ d/f.txt", []string{"d/f.txt", "d/bak/f.txt"}, false},
		{"sed -Ei s/a/b/ f.txt", []string{"f.txt"}, false},
		{"sed -ni p f.txt", []string{"f.txt"}, false},
		{"sed -e 's/x/y/' -i f.txt", []string{"f.txt"}, false},
		{"sed -i -e s/a/b/ -f script.sed f.txt g.txt", []string{"f.txt", "g.txt"}, false},
		{"sed --expression=s/a/b/ --in-place f.txt", []string{"f.txt"}, false},
		{"sed -i -- s/a/b/ f.txt", []string{"f.txt"}, false},
		{"sed -i 's/$/x/' f.txt", []string{"f.txt"}, false}, // a $ in the script is not a path
		{"sed -n p f.txt", nil, false},
		{"sed s/a/b/ f.txt", nil, false},
		{"sed -e s/a/b/ f.txt", nil, false},
		{"sed s/a/b/ f.txt > out.txt", []string{"out.txt"}, false},
		// tee: every file operand, appending or not.
		{"tee docs/other.md", []string{"docs/other.md"}, false},
		{"echo x | tee -a log.txt other.txt", []string{"log.txt", "other.txt"}, false},
		{"echo x | tee --append log.txt", []string{"log.txt"}, false},
		{"echo x | tee -ai log.txt", []string{"log.txt"}, false},
		{"echo x | tee", nil, false},
		{"echo x | tee /dev/null", nil, false},
		// cp: the destination, or DIR/basename for each source.
		{"cp a docs/other.md", []string{"docs/other.md"}, false},
		{"cp -r src dst/", []string{"dst/src"}, false},
		{"cp a b dir", []string{"dir/a", "dir/b"}, false},
		{"cp -t dir a b/c", []string{"dir/a", "dir/c"}, false},
		{"cp --target-directory=dir a", []string{"dir/a"}, false},
		{"cp --target-directory dir a", []string{"dir/a"}, false},
		{"cp -S .old a b", []string{"b"}, false},
		{"cp a /dev/null", nil, false},
		{`cp a "my dir/file name.txt"`, []string{"my dir/file name.txt"}, false},
		{"cp $SRC docs/x", []string{"docs/x"}, false}, // the source is not written
		{"cp only-one-operand", nil, false},
		// mv: the destination, and every source, which it removes.
		{"mv a docs/other.md", []string{"docs/other.md", "a"}, false},
		{"mv a b c dir/", []string{"dir/a", "dir/b", "dir/c", "a", "b", "c"}, false},
		{"mv -t dir a", []string{"dir/a", "a"}, false},
		// Unresolvable operands are refused exactly as a `> $VAR` target is.
		{"cp a $DEST", nil, true},
		{"tee ~/x", nil, true},
		{`sed -i s/a/b/ "$F"`, nil, true},
		{"mv $SRC docs/x", []string{"docs/x"}, true},
		{"cp a b $DIR", nil, true},
		{"cp -t $DIR a", nil, true},
		{"sed -i'$X' s/a/b/ f.txt", []string{"f.txt"}, true},
		{"cp a $(pick)", nil, true},
		// A glob is judged as the literal path, as a glob in `> src/*.go` is.
		{"sed -i s/a/b/ src/*.go", []string{"src/*.go"}, false},
		// Repeats are judged once.
		{"tee f f && sed -i s/a/b/ f", []string{"f"}, false},
		{"echo x > f; tee f", []string{"f"}, false},
		// Every clause, and inside the substitutions the scanner descends into.
		{"true && sed -i s/a/b/ a.txt || tee b.txt; cp x c.txt | cat", []string{"a.txt", "b.txt", "c.txt"}, false},
		{"echo $(sed -i s/a/b/ inner.txt)", []string{"inner.txt"}, false},
		{"echo `tee inner.txt`", []string{"inner.txt"}, false},
		{"cat <(cp a inner.txt)", []string{"inner.txt"}, false},
		{"echo $(echo x; tee inner.txt) outer", []string{"inner.txt"}, false},
		{"(tee f)", []string{"f"}, false},
		{"{ tee f; }", []string{"f"}, false},
		{"if true; then tee f; fi", []string{"f"}, false},
		// Wrappers, assignments and leading redirections precede the command word.
		{"sudo sed -i s/a/b/ f", []string{"f"}, false},
		{"env LC_ALL=C sed -i s/a/b/ f", []string{"f"}, false},
		{"command tee f", []string{"f"}, false},
		{"FOO=1 tee f", []string{"f"}, false},
		{"/usr/bin/tee f", []string{"f"}, false},
		{`"tee" f`, []string{"f"}, false},
		{"2>/dev/null tee f", []string{"f"}, false},
		{"tee f 2>&1", []string{"f"}, false},
		{"tee < in.txt f", []string{"f"}, false},
		{"tee f > g", []string{"g", "f"}, false},
		// Digits glued to a word are part of it, not a descriptor.
		{"tee a2>f", []string{"f", "a2"}, false},
		// A dangling -t is an error cp reports; it writes nothing, and above
		// all not "/a".
		{"cp a b/ -t", nil, false},
		{"mv a --target-directory", nil, false},
		// Mentioning a command is not running it.
		{"echo sed -i s/a/b/ f", nil, false},
		{"grep tee f", nil, false},
		{"echo 'tee f'", nil, false},
	}
	for _, tc := range tests {
		targets, opaque, err := RedirectTargets(tc.input)
		if err != nil {
			t.Errorf("RedirectTargets(%q): unexpected error %v", tc.input, err)
			continue
		}
		if opaque != tc.opaque {
			t.Errorf("RedirectTargets(%q): opaque=%v want %v", tc.input, opaque, tc.opaque)
		}
		if strings.Join(targets, "|") != strings.Join(tc.targets, "|") {
			t.Errorf("RedirectTargets(%q) = %q, want %q", tc.input, targets, tc.targets)
		}
	}
}

// The bound applies to the combined set: a line naming more files than
// maxRedirectTargets through arguments is an incomplete enumeration, as it is
// through redirections, and reports opacity rather than a judged prefix.
func TestArgumentWriteTargetsShareTheBound(t *testing.T) {
	var b strings.Builder
	b.WriteString("tee")
	for i := 0; i <= maxRedirectTargets; i++ {
		fmt.Fprintf(&b, " f%d", i)
	}
	targets, opaque, err := RedirectTargets(b.String())
	if err != nil {
		t.Fatalf("RedirectTargets: %v", err)
	}
	if !opaque {
		t.Errorf("%d tee operands reported complete (%d targets); want opaque", maxRedirectTargets+1, len(targets))
	}
	if len(targets) > maxRedirectTargets {
		t.Errorf("reported %d targets, above the bound %d", len(targets), maxRedirectTargets)
	}
}
