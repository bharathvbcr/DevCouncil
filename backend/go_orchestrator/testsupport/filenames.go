// devcouncil: allow-unwired
//
// Test-only by design, and shared with Manvi. See the package doc in
// testsupport.go for why this is a declaration rather than a finding.
package testsupport

import (
	"runtime"
	"strings"
	"testing"
)

// RequireFileNameHere skips the calling test, or subtest, when the file name
// rel (slash-separated) cannot exist on this platform, and returns otherwise.
//
// A hostile-name table (tab, newline, quote, trailing dot) is written for
// POSIX, where a name is any bytes but "/" and NUL. Windows refuses to create a
// file whose name has a control character or one of < > : " | ? *, or whose
// component ends in a dot or a space, so the failure there is the fixture
// failing to exist ("The filename, directory name, or volume label syntax is
// incorrect") and not the code under test mishandling the name. The skip says
// which name and why, and only that case is skipped: the rest of the table
// keeps running.
func RequireFileNameHere(t testing.TB, rel string) {
	t.Helper()
	if runtime.GOOS != "windows" {
		return
	}
	if reason := windowsNameProblem(rel); reason != "" {
		t.Skipf("file name %q cannot exist on Windows: %s", rel, reason)
	}
}

// windowsNameProblem says why rel is not a creatable Windows path, or "" when
// it is. It is pure, so it is tested on every platform.
func windowsNameProblem(rel string) string {
	for _, part := range strings.Split(rel, "/") {
		for _, r := range part {
			switch {
			case r < 0x20:
				return "a control character is not allowed in a name"
			case strings.ContainsRune(`<>:"|?*`, r):
				return "the character " + string(r) + " is not allowed in a name"
			}
		}
		if part != "" && part != "." && part != ".." && (strings.HasSuffix(part, ".") || strings.HasSuffix(part, " ")) {
			return "a name may not end in a dot or a space"
		}
	}
	return ""
}
