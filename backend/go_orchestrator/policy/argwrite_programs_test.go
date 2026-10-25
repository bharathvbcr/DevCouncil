package policy

import (
	"reflect"
	"sort"
	"testing"
)

func sorted(in []string) []string {
	out := append([]string(nil), in...)
	sort.Strings(out)
	if len(out) == 0 {
		return nil
	}
	return out
}

// The argument rung names what each program writes or removes from its words
// alone. Each row is one reading a refusal depends on: a combined flag, an
// attached value, `--`, an option placed after the operands, a repeated option.
func TestArgumentWriteTargetsNameWhatEachProgramWrites(t *testing.T) {
	rows := []struct {
		line     string
		targets  []string
		deletes  []string
		opaque   bool
		unjudged bool
	}{
		// curl
		{line: "curl -o .env http://h/x", targets: []string{".env"}},
		{line: "curl -fo .env http://h/x", targets: []string{".env"}},
		{line: "curl -fsSLo.env http://h/x", targets: []string{".env"}},
		{line: "curl --output=.env http://h/x", targets: []string{".env"}},
		{line: "curl http://h/x --output .env", targets: []string{".env"}},
		{line: "curl -o a http://h/a -o .env http://h/b", targets: []string{".env", "a"}},
		{line: "curl -O http://h/p/.env", targets: []string{".env"}},
		{line: "curl -O 'http://h/p/.env?x=1#f'", targets: []string{".env"}},
		{line: "curl --output-dir .git/hooks -O http://h/pre-commit", targets: []string{".git/hooks/pre-commit"}},
		{line: "curl --output-dir .git/hooks -o pre-commit http://h/x", targets: []string{".git/hooks/pre-commit"}},
		{line: "curl -D .env http://h/x", targets: []string{".env"}},
		{line: "curl -c .env http://h/x", targets: []string{".env"}},
		{line: "curl -o - http://h/x"},
		{line: "curl -sSL http://h/x"},
		{line: "curl -H 'X-A: b' http://h/x"},
		{line: "curl -o \"$F\" http://h/x", targets: []string{"$F"}},
		{line: "curl -o \"$(pick)\" http://h/x", opaque: true},
		{line: "curl -K cfg http://h/x", unjudged: true},
		// wget
		{line: "wget -O .env http://h/x", targets: []string{".env"}},
		{line: "wget -qO .env http://h/x", targets: []string{".env"}},
		{line: "wget -O.env http://h/x", targets: []string{".env"}},
		{line: "wget --output-document=.env http://h/x", targets: []string{".env"}},
		{line: "wget --output-document .env http://h/x", targets: []string{".env"}},
		{line: "wget -O - http://h/x"},
		{line: "wget -qO- http://h/x"},
		{line: "wget http://h/a/.env", targets: []string{".env"}},
		{line: "wget http://h/", targets: []string{"index.html"}},
		{line: "wget -P .git/hooks http://h/a/pre-commit", targets: []string{".git/hooks/pre-commit"}},
		{line: "wget -o .env http://h/x -O -", targets: []string{".env"}},
		{line: "wget -r http://h/x", targets: []string{"x"}, unjudged: true},
		{line: "wget -e dir_prefix=.git/hooks http://h/x", targets: []string{"x"}, unjudged: true},
		// in-place interpreters
		{line: "ruby -pi -e x .env", targets: []string{".env"}},
		{line: "ruby -pi.bak -e x f", targets: []string{"f", "f.bak"}},
		{line: "ruby -i.bak -pe x f", targets: []string{"f", "f.bak"}},
		{line: "ruby -e x f"},
		{line: "ruby -C d -pi -e x f", targets: []string{"f"}, unjudged: true},
		{line: "perl -pi -e x .env", targets: []string{".env"}},
		// removals
		{line: "rm .env", targets: []string{".env"}, deletes: []string{".env"}},
		{line: "rm -- -f", targets: []string{"-f"}, deletes: []string{"-f"}},
		{line: "rm -rf -- .git", targets: []string{".git"}, deletes: []string{".git"}},
		{line: "rmdir .git", targets: []string{".git"}, deletes: []string{".git"}},
		{line: "unlink .env", targets: []string{".env"}, deletes: []string{".env"}},
		{line: "shred -u .env", targets: []string{".env"}, deletes: []string{".env"}},
		{line: "shred --remove=wipe .env", targets: []string{".env"}, deletes: []string{".env"}},
		{line: "shred -n 3 .env", targets: []string{".env"}},
		{line: "rm a; touch a", targets: []string{"a", "a"}, deletes: []string{"a"}},
		// metadata and creation
		{line: "mkdir -p .git/hooks/x", targets: []string{".git/hooks/x"}},
		{line: "chmod +x .git/hooks/pre-commit", targets: []string{".git/hooks/pre-commit"}},
		{line: "chmod -x f", targets: []string{"f"}},
		{line: "chmod -R 755 a b", targets: []string{"a", "b"}},
		{line: "chmod --reference=r f", targets: []string{"f"}},
		{line: "chown -R me a", targets: []string{"a"}},
		// git
		{line: "git checkout -- .env", targets: []string{".env"}},
		{line: "git checkout HEAD -- a b", targets: []string{"a", "b"}},
		{line: "git checkout main"},
		{line: "git checkout -b feature"},
		{line: "git checkout .env", targets: []string{".env"}},
		{line: "git checkout main src/a.go", targets: []string{"src/a.go"}},
		{line: "git checkout ../x", targets: []string{"../x"}},
		{line: "git restore .env", targets: []string{".env"}},
		{line: "git restore --source=HEAD~1 -- .env", targets: []string{".env"}},
		{line: "git restore -s HEAD -SW -- .env", targets: []string{".env"}},
		{line: "git restore --staged .env"},
		{line: "git rm -f -- .env", targets: []string{".env"}, deletes: []string{".env"}},
		{line: "git rm --cached .env"},
		{line: "git mv a .env", targets: []string{".env", "a"}},
		{line: "git clean -fd .env", targets: []string{".env"}, deletes: []string{".env"}},
		{line: "git clean -n"},
		{line: "git clean -fd", unjudged: true},
		{line: "git -c core.x=1 checkout -- .env", targets: []string{".env"}},
		// wrappers
		{line: "xargs rm .env", targets: []string{".env"}, deletes: []string{".env"}, unjudged: true},
		{line: "find . -exec rm .env ;", targets: []string{".env"}, deletes: []string{".env"}},
		{line: "sudo rm .env", targets: []string{".env"}, deletes: []string{".env"}},
	}
	for _, row := range rows {
		got := argumentWriteTargets(row.line)
		if !reflect.DeepEqual(sorted(got.targets), sorted(row.targets)) ||
			!reflect.DeepEqual(sorted(got.deletes), sorted(row.deletes)) ||
			got.opaque != row.opaque || (got.unjudged != "") != row.unjudged {
			t.Errorf("%q: targets %q deletes %q opaque=%v unjudged=%q\n  want targets %q deletes %q opaque=%v unjudged=%v",
				row.line, got.targets, got.deletes, got.opaque, got.unjudged,
				row.targets, row.deletes, row.opaque, row.unjudged)
		}
	}
}

// A path that is both removed and written in one line is judged as the write:
// the stricter reading of the two.
func TestRedirectWritesDeletesAreOnlyRemovals(t *testing.T) {
	for _, tc := range []struct {
		line    string
		deletes []string
	}{
		{"rm a", []string{"a"}},
		{"rm a; touch a", nil},
		{"rm a && echo x > a", nil},
		{"rm a b; touch b", []string{"a"}},
		{"echo $(rm a)", []string{"a"}},
		{"git rm -q a && git clean -fd b", []string{"a", "b"}},
	} {
		set, err := RedirectWrites(tc.line)
		if err != nil {
			t.Fatal(err)
		}
		if !reflect.DeepEqual(sorted(set.Deletes), sorted(tc.deletes)) {
			t.Errorf("%q: Deletes = %q, want %q (Targets %q)", tc.line, set.Deletes, tc.deletes, set.Targets)
		}
		for _, d := range set.Deletes {
			if !containsString(set.Targets, d) {
				t.Errorf("%q: delete %q is missing from Targets %q; callers that ignore Deletes would miss it", tc.line, d, set.Targets)
			}
		}
	}
}
