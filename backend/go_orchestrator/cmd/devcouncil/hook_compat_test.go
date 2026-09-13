package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestRetiredHookNeverStartsDependencies(t *testing.T) {
	_, argv := fakeDevmap(t)
	for _, event := range []string{"session-start", "post-tool-use", "session-end", "user-prompt-submit", "agent-response", "pre-tool-use"} {
		out, restoreOut := swapStdout(t)
		errOut, restoreErr := swapStderr(t)
		code := dispatch([]string{"hook", event, "--project-root", filepath.Join(t.TempDir(), "absent"), "--client", "claude"})
		restoreErr()
		restoreOut()
		if code != 0 || out.Len() != 0 || errOut.Len() != 0 {
			t.Fatalf("%s must be silent: exit %d out=%q err=%q", event, code, out.String(), errOut.String())
		}
		if _, err := os.Stat(argv); !os.IsNotExist(err) {
			t.Fatalf("%s started DevMap (stat=%v)", event, err)
		}
	}
}

func TestDisableHooksDryRunDoesNotMutate(t *testing.T) {
	root := t.TempDir()
	t.Setenv("DEVCOUNCIL_PROJECT_ROOT", root)
	path := filepath.Join(root, ".claude/settings.local.json")
	if err := os.MkdirAll(filepath.Dir(path), 0700); err != nil {
		t.Fatal(err)
	}
	original := `{"hooks":{"Stop":[{"hooks":[{"command":"dev hook agent-response"}]}]}}`
	if err := os.WriteFile(path, []byte(original), 0600); err != nil {
		t.Fatal(err)
	}
	out, restoreOut := swapStdout(t)
	errOut, restoreErr := swapStderr(t)
	code := dispatch([]string{"disable", "hooks", "--dry-run"})
	restoreErr()
	restoreOut()
	if code != 0 {
		t.Fatalf("exit %d: %s", code, errOut.String())
	}
	b, err := os.ReadFile(path)
	if err != nil || string(b) != original {
		t.Fatalf("dry-run mutated file: %s %v", b, err)
	}
	if !strings.Contains(out.String(), "would_clean") {
		t.Fatalf("missing preview: %s", out.String())
	}
}
