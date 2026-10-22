package policy

import (
	"reflect"
	"strings"
	"testing"
)

// The files rm, truncate, touch, install, ln, rsync, patch, dd and perl -i
// write, read from their arguments, and the commands whose writes are not in
// their text. Targets are in the order RedirectWrites reports them.
func TestFileWriterTargets(t *testing.T) {
	tests := []struct {
		input    string
		targets  []string
		opaque   bool
		unjudged bool
	}{
		{"rm a b", []string{"a", "b"}, false, false},
		{"rm -rf -- -weird dir/", []string{"-weird", "dir/"}, false, false},
		{"rm -f", nil, false, false},
		{"rm $X", nil, true, false},
		{"rm -rf ~/x", nil, true, false},
		{"rm -r build/*", []string{"build/*"}, false, true},
		{"rm build/*", []string{"build/*"}, false, false},
		{"truncate -s 0 a b", []string{"a", "b"}, false, false},
		{"truncate -r ref a", []string{"a"}, false, false},
		{"truncate --size=1K a", []string{"a"}, false, false},
		{"touch a", []string{"a"}, false, false},
		{"touch -r ref -d 'now' a b", []string{"a", "b"}, false, false},
		{"touch -c -t 202001010000 a", []string{"a"}, false, false},
		{"touch -", nil, false, false},
		{"install a b", []string{"b"}, false, false},
		{"install -m 644 -o root a dir/", []string{"dir/a"}, false, false},
		{"install -m644 a b", []string{"b"}, false, false},
		{"install -t dir a b", []string{"dir/a", "dir/b"}, false, false},
		{"install -d x y/z", []string{"x", "y/z"}, false, false},
		{"ln -s a b", []string{"b"}, false, false},
		{"ln -sf a dir/", []string{"dir/a"}, false, false},
		{"ln -s ../a", []string{"./a"}, false, false},
		{"ln -t dir a b", []string{"dir/a", "dir/b"}, false, false},
		{"rsync -a a b", []string{"b"}, false, false},
		{"rsync -a src/ dest/", []string{"dest"}, false, false},
		{"rsync -a a b dest/", []string{"dest/a", "dest/b"}, false, false},
		{"rsync -a a host:dir", nil, false, false},
		{"rsync -a host:a dest", []string{"dest"}, false, false},
		{"rsync -e ssh --exclude x a b", []string{"b"}, false, false},
		{"rsync --backup-dir=bk a b", []string{"bk", "b"}, false, false},
		{"rsync --remove-source-files a b", []string{"a", "b"}, false, false},
		{"rsync -a a $D", nil, true, false},
		{"patch f fix.diff", []string{"f"}, false, false},
		{"patch f < fix.diff", []string{"f"}, false, false},
		{"patch -p1 -o out < fix.diff", []string{"out"}, false, false},
		{"patch -p1 < fix.diff", nil, false, true},
		{"patch -p1 -i fix.diff", nil, false, true},
		{"patch -d sub f", nil, false, true},
		{"patch --dry-run -p1 < fix.diff", nil, false, false},
		{"dd if=a of=b", []string{"b"}, false, false},
		{"dd if=a of=$X", nil, true, false},
		{"dd if=a", nil, false, false},
		{"perl -pi -e s/a/b/ f", []string{"f"}, false, false},
		{"perl -i.bak -pe s/a/b/ f", []string{"f", "f.bak"}, false, false},
		{"perl -0777 -pi -e s/a/b/ f g", []string{"f", "g"}, false, false},
		{"perl -i script.pl f", []string{"f"}, false, false},
		{"perl -pe s/a/b/ f", nil, false, false},
		{"perl -i -e 1 $F", nil, true, false},
		{"git apply fix.diff", nil, false, true},
		{"git -C d apply --index fix.diff", nil, false, true},
		{"git apply --check fix.diff", nil, false, false},
		{"git apply --stat --apply fix.diff", nil, false, true},
		{"git status", nil, false, false},
		{"xargs rm", nil, false, true},
		{"xargs -0 -n 1 rm -f", nil, false, true},
		{"xargs -I{} cp {} out/", nil, false, true},
		{"xargs -I % mv % dest/", nil, false, true},
		{"xargs cp -t dest", []string{}, false, true},
		{"xargs grep -l x", nil, false, false},
		{"xargs -n1 echo", nil, false, false},
		{"xargs", nil, false, false},
		{"xargs xargs rm", nil, false, true},
		{"find . -delete", nil, false, true},
		{"find . -exec rm {} \\;", nil, false, true},
		{"find . -exec cp -t backup/ {} +", nil, false, true},
		{"find . -exec cp seed.txt out.txt \\;", []string{"out.txt"}, false, false},
		{"find . -exec grep x {} +", nil, false, false},
		{"find . -fprint list.txt", []string{"list.txt"}, false, false},
		{"find . -name x", nil, false, false},
		{"echo $(find . -delete)", nil, false, true},
		{"sudo rm a", []string{"a"}, false, false},
	}
	for _, tc := range tests {
		set, err := RedirectWrites(tc.input)
		if err != nil {
			t.Errorf("RedirectWrites(%q): %v", tc.input, err)
			continue
		}
		got := set.Targets
		if len(got) == 0 && len(tc.targets) == 0 {
			got = tc.targets
		}
		if !reflect.DeepEqual(got, tc.targets) || set.Opaque != tc.opaque || (set.Unjudged != "") != tc.unjudged {
			t.Errorf("RedirectWrites(%q) = %q opaque=%v unjudged=%q; want %q opaque=%v unjudged=%v",
				tc.input, set.Targets, set.Opaque, set.Unjudged, tc.targets, tc.opaque, tc.unjudged)
		}
	}
}

// A line whose commands write files it does not name must say so however it is
// wrapped, or a scope could be walked around by quoting the command.
func TestUnjudgedWritesSurviveWrapping(t *testing.T) {
	for _, command := range []string{
		"true && xargs rm",
		"cat list | xargs rm",
		"if true; then find . -delete; fi",
		"(git apply fix.diff)",
		"env xargs rm",
		"FOO=1 find . -delete",
		"echo \"$(xargs rm)\"",
	} {
		set, err := RedirectWrites(command)
		if err != nil {
			t.Fatal(err)
		}
		if set.Unjudged == "" {
			t.Errorf("%q: no unjudged write reported (targets %v)", command, set.Targets)
		}
	}
}

// The write bound still holds: a line of many distinct removals is refused as
// incomplete rather than judged in part.
func TestFileWritersRespectTheTargetBound(t *testing.T) {
	var b strings.Builder
	b.WriteString("rm")
	for i := 0; i < maxRedirectTargets+5; i++ {
		b.WriteString(" f")
		b.WriteString(strings.Repeat("x", i+1))
	}
	set, err := RedirectWrites(b.String())
	if err != nil {
		t.Fatal(err)
	}
	if !set.Opaque || len(set.Targets) != maxRedirectTargets {
		t.Errorf("got %d targets opaque=%v, want %d and opaque", len(set.Targets), set.Opaque, maxRedirectTargets)
	}
}
