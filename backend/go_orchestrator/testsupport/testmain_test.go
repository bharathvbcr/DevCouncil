package testsupport

import (
	"fmt"
	"os"
	"testing"
)

// TestMain clears the prebuilt-binary variables before any test runs. This
// package's tests exercise the build path itself — a fake cargo that must be
// called, a real `cargo metadata` — and a gate that exports NoCargoEnv for the
// packages it tests would otherwise make them refuse to build. The tests of the
// mode set what they need through t.Setenv, which restores this cleared state.
func TestMain(m *testing.M) {
	for _, name := range []string{NoCargoEnv, "DCSTORE_BIN", "DCVERIFY_BIN", "DCGREP_BIN", "DEVMAP_BIN"} {
		if err := os.Unsetenv(name); err != nil {
			fmt.Fprintf(os.Stderr, "unset %s: %v\n", name, err)
			os.Exit(2)
		}
	}
	os.Exit(m.Run())
}
