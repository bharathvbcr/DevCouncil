package main

// install and uninstall moved to devcouncil/components (cli.go), with their
// tests; disable/enable stay here because `hooks` is the host's to manage.

import "testing"

func TestDisableRefusesAnUnknownComponentAndAMissingName(t *testing.T) {
	prefix := t.TempDir()
	_, restore := swapStderr(t)
	unknown := runDisable([]string{"uv", "--prefix", prefix})
	missing := runDisable([]string{"--prefix", prefix})
	restore()
	if unknown == 0 || missing == 0 {
		t.Fatalf("disable uv exited %d, disable with no name exited %d", unknown, missing)
	}
}
