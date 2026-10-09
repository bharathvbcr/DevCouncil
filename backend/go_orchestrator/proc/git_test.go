package proc

import (
	"os"
	"slices"
	"testing"
)

func TestGitArgsPlacesGuards(t *testing.T) {
	global := []string{
		"-c", "core.fsmonitor=false",
		"-c", "core.hooksPath=" + os.DevNull,
		"-c", "log.showSignature=false",
	}
	cases := []struct {
		name string
		in   []string
		want []string
	}{
		{"diff gets both", []string{"diff", "HEAD"},
			[]string{"diff", "--no-ext-diff", "--no-textconv", "HEAD"}},
		{"blame gets textconv only", []string{"blame", "--line-porcelain", "HEAD", "--", "a"},
			[]string{"blame", "--no-textconv", "--line-porcelain", "HEAD", "--", "a"}},
		{"log and show", []string{"show", "--name-only", "x"},
			[]string{"show", "--no-ext-diff", "--no-textconv", "--name-only", "x"}},
		{"caller globals stay before the subcommand",
			[]string{"-c", "core.quotePath=false", "-C", "sub", "--no-pager", "diff", "--unified=0"},
			[]string{"-c", "core.quotePath=false", "-C", "sub", "--no-pager", "diff", "--no-ext-diff", "--no-textconv", "--unified=0"}},
		{"a subcommand without guards is unchanged", []string{"ls-files", "-z"}, []string{"ls-files", "-z"}},
		{"an argument after the subcommand named diff is not a subcommand",
			[]string{"rev-parse", "diff"}, []string{"rev-parse", "diff"}},
		{"a dangling -c is kept, not overrun", []string{"-c"}, []string{"-c"}},
		{"flags only", []string{"--version"}, []string{"--version"}},
		{"empty", nil, nil},
	}
	for _, c := range cases {
		got := GitArgs(c.in)
		want := append(slices.Clone(global), c.want...)
		if !slices.Equal(got, want) {
			t.Errorf("%s: GitArgs(%q)\n got %q\nwant %q", c.name, c.in, got, want)
		}
	}
}

// The caller's slice is not modified: guards go into a new one.
func TestGitArgsDoesNotAliasItsInput(t *testing.T) {
	in := make([]string, 2, 16)
	in[0], in[1] = "diff", "HEAD"
	_ = GitArgs(in)
	if !slices.Equal(in, []string{"diff", "HEAD"}) || !slices.Equal(in[:cap(in)][2:4], []string{"", ""}) {
		t.Fatalf("input changed: %q", in[:cap(in)])
	}
}
