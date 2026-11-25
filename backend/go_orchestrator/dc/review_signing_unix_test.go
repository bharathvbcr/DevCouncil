//go:build !windows

package dc

import (
	"os"
	"testing"
)

// writeLooseKeyFile writes a key file that others can read: mode 0644.
func writeLooseKeyFile(t *testing.T, path string, data []byte) {
	t.Helper()
	if err := os.WriteFile(path, data, 0o644); err != nil {
		t.Fatal(err)
	}
}
