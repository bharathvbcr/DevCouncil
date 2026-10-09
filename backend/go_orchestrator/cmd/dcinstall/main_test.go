package main

import (
	"os"
	"os/exec"
	"strings"
	"testing"
)

func TestAnUnknownVerbIsAUsageError(t *testing.T) {
	if code := run([]string{"reinstall"}); code != 2 {
		t.Fatalf("exit %d", code)
	}
}

func TestUninstallIsDispatchedAndStillAsksForConfirmation(t *testing.T) {
	if code := run([]string{"uninstall", "devmap", "--prefix", t.TempDir()}); code != 2 {
		t.Fatalf("exit %d", code)
	}
}

// The point of this command: it builds where the host cannot, with no
// Gusset checkout beside DevCouncil.
func TestItBuildsWithoutGusset(t *testing.T) {
	if testing.Short() {
		t.Skip("builds a binary")
	}
	out, err := exec.Command("go", "list", "-deps", ".").CombinedOutput()
	if err != nil {
		t.Fatalf("go list: %v\n%s", err, out)
	}
	if strings.Contains(string(out), "gusset") {
		t.Fatalf("dcinstall depends on Gusset:\n%s", out)
	}
	cmd := exec.Command("go", "build", "-o", os.DevNull, ".")
	cmd.Env = append(os.Environ(), "GOWORK=off")
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("GOWORK=off go build: %v\n%s", err, out)
	}
}
