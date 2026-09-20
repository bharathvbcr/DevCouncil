package main

import (
	"encoding/json"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/dcgrep"
)

func TestDispatchGrepWithoutPatternExits2(t *testing.T) {
	stderr, restore := swapStderr(t)
	code := dispatch([]string{"grep"})
	restore()
	if code != 2 {
		t.Fatalf("exit %d, want 2 stderr=%s", code, stderr.String())
	}
	if !strings.Contains(stderr.String(), "PATTERN") {
		t.Fatalf("stderr=%q", stderr.String())
	}
}

func TestDispatchGrepIsNotAnUnknownCommand(t *testing.T) {
	stderr, restore := swapStderr(t)
	code := dispatch([]string{"grep", "--help"})
	restore()
	if code != 0 {
		t.Fatalf("exit %d, want 0 stderr=%s", code, stderr.String())
	}
	if strings.Contains(stderr.String(), "unknown command") {
		t.Fatalf("grep was rejected as unknown: %s", stderr.String())
	}
}

func TestDispatchGrepMissingBinaryIsAnErrorNotAnEmptyResult(t *testing.T) {
	t.Setenv(dcgrep.BinaryEnv, "")
	t.Setenv("PATH", t.TempDir())
	stdout, restoreOut := swapStdout(t)
	stderr, restoreErr := swapStderr(t)
	code := dispatch([]string{"grep", "--json", "needle", "--project-root", t.TempDir()})
	restoreErr()
	restoreOut()
	if code != 1 {
		t.Fatalf("exit %d, want 1 stdout=%s stderr=%s", code, stdout.String(), stderr.String())
	}
	if strings.Contains(stdout.String(), `"count"`) {
		t.Fatalf("missing binary wrote a search result: %s", stdout.String())
	}
	if !strings.Contains(stderr.String(), "dcgrep") {
		t.Fatalf("stderr does not name the missing binary: %s", stderr.String())
	}
}

func TestDispatchGrepUsesTheClientAgainstANamedBinary(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("the fake searcher is a shell script")
	}
	body := `#!/bin/sh
echo '{"ok":true,"pattern":"needle","count":1,"matches":[{"path":"a.go","line_number":3,"line":"needle here"}],"files_searched":1,"skipped":{"too_large":0,"binary":0,"unreadable":0,"unrepresentable_name":0},"ignore_rules_applied":true}'
`
	bin := filepath.Join(t.TempDir(), "dcgrep")
	if err := os.WriteFile(bin, []byte(body), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv(dcgrep.BinaryEnv, bin)
	stdout, restoreOut := swapStdout(t)
	stderr, restoreErr := swapStderr(t)
	code := dispatch([]string{"grep", "--json", "needle", "--project-root", t.TempDir()})
	restoreErr()
	restoreOut()
	if code != 0 {
		t.Fatalf("exit %d stdout=%s stderr=%s", code, stdout.String(), stderr.String())
	}
	var payload map[string]any
	if err := json.Unmarshal(stdout.Bytes(), &payload); err != nil {
		t.Fatalf("stdout is not JSON: %v (%q)", err, stdout.String())
	}
	if payload["count"] != float64(1) {
		t.Fatalf("count=%v stdout=%s", payload["count"], stdout.String())
	}
}
