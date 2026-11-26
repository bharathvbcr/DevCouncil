package devmap

import (
	"path/filepath"
	"runtime"
	"testing"
)

// TestPlainPathStripsOnlyTheVerbatimPrefixesThatNameADriveOrShare pins the
// conversion of DevMap's Windows spelling. The drive and share forms mean the
// same file once the prefix is gone; any other verbatim form names something
// the prefix is part of, and rewriting it would point at a different file.
func TestPlainPathStripsOnlyTheVerbatimPrefixesThatNameADriveOrShare(t *testing.T) {
	for _, c := range []struct{ in, want string }{
		{`\\?\C:\work\x`, `C:\work\x`},
		{`\\?\c:\`, `c:\`},
		{`\\?\C:`, `C:`},
		{`\\?\UNC\host\share\x`, `\\host\share\x`},
		{`C:\work\x`, `C:\work\x`},
		{`/home/agent/x`, `/home/agent/x`},
		{`relative/x`, `relative/x`},
		{``, ``},
		{`\\?\`, `\\?\`},
		{`\\?\UNC\`, `\\?\UNC\`},
		{`\\?\Volume{0123}\x`, `\\?\Volume{0123}\x`},
		{`\\?\1:\x`, `\\?\1:\x`},
		{`\\.\pipe\x`, `\\.\pipe\x`},
		{`\\server\share\x`, `\\server\share\x`},
	} {
		if got := PlainPath(c.in); got != c.want {
			t.Errorf("PlainPath(%q) = %q, want %q", c.in, got, c.want)
		}
	}
}

// TestRelativeToPassesAPathOutsideTheRootThroughWhole: a path the root does not
// contain is not a repository path, so it comes back as given, never as one
// climbing out with `..`. On Windows filepath.Rel spells that climb `..\`,
// which a "../" check missed: integrate's receipts were keyed
// ../../../../../../runneradmin/... when the root was the 8.3 spelling of the
// same temp directory.
func TestRelativeToPassesAPathOutsideTheRootThroughWhole(t *testing.T) {
	base := t.TempDir()
	outside := filepath.Join(base, "other", "x.md")
	if got := RelativeTo(filepath.Join(base, "repo"), outside); got != outside {
		t.Fatalf("RelativeTo = %q, want the path itself, %q", got, outside)
	}
	inside := filepath.Join(base, "repo", ".agents", "a.md")
	if got := RelativeTo(filepath.Join(base, "repo"), inside); got != ".agents/a.md" {
		t.Fatalf("RelativeTo = %q, want .agents/a.md", got)
	}
}

// TestRelativeToReadsAVerbatimReportAgainstAPlainRoot is the Windows failure
// that was reported as `skills install` naming files by an absolute path: the
// producer answered `\\?\C:\...` and the caller's root was `C:\...`, so no
// relation was found and the path was passed through whole.
func TestRelativeToReadsAVerbatimReportAgainstAPlainRoot(t *testing.T) {
	if runtime.GOOS != "windows" {
		t.Skip("a backslash is a separator only on Windows, so this spelling cannot relate to its root elsewhere")
	}
	got := RelativeTo(`C:\work\repo`, `\\?\C:\work\repo\.claude\skills\core\SKILL.md`)
	if want := `.claude/skills/core/SKILL.md`; got != want {
		t.Fatalf("RelativeTo = %q, want %q", got, want)
	}
}
