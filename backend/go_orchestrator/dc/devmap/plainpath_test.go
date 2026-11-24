package devmap

import (
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
