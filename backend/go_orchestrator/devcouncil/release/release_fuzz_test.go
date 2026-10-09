package release

import (
	"strings"
	"testing"
)

// The gate parses files by hand, so every parser is fuzzed for the two
// failures a hand parser has: a panic on input it did not foresee, and an
// answer that silently drops part of what it was given.

// An array written from known strings reads back as exactly those strings,
// whichever TOML string form each was written in.
func FuzzStringArrayRoundTrip(f *testing.F) {
	f.Add("dc-store", "tools/soak", "it's", uint8(0))
	f.Add(`say "hi"`, "a]b", "#not-a-comment", uint8(5))
	f.Add("", "x,y", `back\slash`, uint8(2))
	f.Fuzz(func(t *testing.T, a, b, c string, styles uint8) {
		values := []string{a, b, c}
		var rendered []string
		for i, v := range values {
			if strings.ContainsAny(v, "\n\r") {
				t.Skip("TOML strings here are single-line")
			}
			literal := styles&(1<<i) != 0 && !strings.Contains(v, "'")
			if literal {
				rendered = append(rendered, "'"+v+"'")
			} else {
				rendered = append(rendered, `"`+strings.NewReplacer(`\`, `\\`, `"`, `\"`).Replace(v)+`"`)
			}
		}
		source := "[workspace]\nmembers = [" + strings.Join(rendered, ", ") + "] # trailing\n"
		got, unreadable := workspaceMembers(source)
		if len(unreadable) != 0 || len(got) != len(values) {
			t.Fatalf("source %q: got %q, unreadable %q", source, got, unreadable)
		}
		for i := range values {
			if got[i] != values[i] {
				t.Fatalf("source %q: element %d = %q, want %q", source, i, got[i], values[i])
			}
		}
	})
}

// Arbitrary bytes never panic any parser, and whatever an array scan reads is
// accounted for: values plus unreadable entries, never a silent nothing for a
// body that holds something other than separators.
func FuzzParsersNeverPanic(f *testing.F) {
	for _, seed := range []string{
		"[workspace]\nmembers = [\"a\", 'b']\n[workspace.package]\nversion = \"1.2.3\"\n",
		"[[package]]\nname = \"x\"\nversion = \"1\"\nsource = \"registry\"\n",
		"[workspace]\nmembers = [\n  \"a\", # c\n  b,\n",
		"if gh release view x; then\n gh release upload x --clobber\n gh release edit x --notes-file n\nelse\nfi",
		"targets = [\"x86_64-pc-windows-msvc\"",
		"\"\\",
	} {
		f.Add(seed)
	}
	f.Fuzz(func(t *testing.T, s string) {
		ParseTomlSectionVersion(s, "[package]")
		ParseCargoLockPackages(s)
		ParseCargoLockVersion(s, "x")
		ParseGoVersion(s)
		_, _ = ParseNpmVersion(s)
		PackageIsDistable(s)
		ReleaseNotesPublishGaps(s)
		NotesAreWithdrawn(s)
		SuccessorVersions(s)
		distWorkspaceTargets(s)
		members, unreadable := stringArray(s)
		body := s
		if i := strings.IndexByte(body, ']'); i >= 0 {
			body = body[:i]
		}
		if len(members) == 0 && len(unreadable) == 0 && strings.Trim(body, " \t\r\n,") != "" && !strings.HasPrefix(strings.TrimLeft(body, " \t\r\n,"), "#") {
			t.Fatalf("%q: an array body with content read as empty and clean", s)
		}
	})
}

// An accepted tag is exactly "v" plus the version it names.
func FuzzParseTag(f *testing.F) {
	for _, seed := range []string{"v0.2.4", "v01.2.3", "0.2.4", "v1.2.3-rc.1", "v1.2", "v99999999999999999999.0.0"} {
		f.Add(seed)
	}
	f.Fuzz(func(t *testing.T, tag string) {
		version, reason := ParseTag(tag)
		if (reason == "") != (version != "") {
			t.Fatalf("%q: version %q with reason %q", tag, version, reason)
		}
		if reason == "" && ("v"+version != tag || !semverRe.MatchString(version)) {
			t.Fatalf("%q accepted as %q", tag, version)
		}
	})
}
