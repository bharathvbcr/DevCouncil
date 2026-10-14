package proc

import (
	"slices"
	"testing"
)

func TestScrubEnvKeepsOnlyTheAllowlistAndTheNamedExtras(t *testing.T) {
	environ := []string{
		"PATH=/usr/bin", "HOME=/home/u", "LC_ALL=C",
		"GITHUB_TOKEN=ghp_x", "AWS_SECRET_ACCESS_KEY=x", "NODE_OPTIONS=--require=/evil.js",
		"WANTED=1", "NOEQUALS", "path=/lowercase/is/another/name",
	}
	got := ScrubEnv(environ, "WANTED")
	want := []string{"PATH=/usr/bin", "HOME=/home/u", "LC_ALL=C", "WANTED=1"}
	if !slices.Equal(got, want) {
		t.Fatalf("ScrubEnv = %q, want %q", got, want)
	}
}

// A nil Cmd.Env inherits everything, so an empty result must still be a
// non-nil slice.
func TestScrubEnvNeverReturnsNil(t *testing.T) {
	for _, environ := range [][]string{nil, {}, {"GITHUB_TOKEN=x"}} {
		if got := ScrubEnv(environ); got == nil || len(got) != 0 {
			t.Fatalf("ScrubEnv(%q) = %#v", environ, got)
		}
	}
}
