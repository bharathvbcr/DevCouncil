// devcouncil: allow-unwired
//
// Test-only by design, and shared with Manvi. See the package doc in
// testsupport.go for why this is a declaration rather than a finding.
package testsupport

import (
	"runtime"
	"testing"
)

// RequireShellFakes skips the calling test on Windows, where a fake binary
// written as a `#!/bin/sh` script cannot be run.
//
// CreateProcess starts executables, not scripts: a file with no executable
// extension is not found on PATH ("executable file not found"), and one named
// *.sh is refused with "%1 is not a valid Win32 application". Neither says
// anything about the code under test, so the tests that stand a shell script in
// for a binary cannot run there as written.
//
// This is a skip and not a pass, on purpose, and it is the only skip on this
// path: it names the fixture, so the uncovered seam reads as a known limit of
// the fixture and not as a toolchain that was silently absent. Giving these
// tests a fake that is a Go program, such as the test binary re-executed, is
// what would let them run on Windows.
//
// Call it from the helper that writes the fake, not from each test, so a test
// that never needs the fake keeps running.
func RequireShellFakes(t testing.TB) {
	t.Helper()
	if runtime.GOOS == "windows" {
		t.Skip("this test stands a shell script in for a binary, and Windows cannot execute one")
	}
}
