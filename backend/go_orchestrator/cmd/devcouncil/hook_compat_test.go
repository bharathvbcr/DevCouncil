package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
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

func TestHookDoesNotWaitForStdin(t *testing.T) {
	old := os.Stdin
	r, w, err := os.Pipe()
	if err != nil {
		t.Fatal(err)
	}
	os.Stdin = r
	defer func() {
		os.Stdin = old
		if err := r.Close(); err != nil {
			t.Error(err)
		}
		if err := w.Close(); err != nil {
			t.Error(err)
		}
	}()
	for _, event := range []string{"session-start", "post-tool-use", "session-end", "user-prompt-submit"} {
		done := make(chan int, 1)
		go func() { done <- dispatch([]string{"hook", event}) }()
		select {
		case code := <-done:
			if code != 0 {
				t.Fatalf("exit %d", code)
			}
		case <-time.After(time.Second):
			t.Fatal("hook waited for stdin")
		}
	}
}

func TestHookStatusAndScopedDisable(t *testing.T) {
	root := t.TempDir()
	for _, rel := range []string{".claude/settings.local.json", ".codex/hooks.json"} {
		path := filepath.Join(root, rel)
		if err := os.MkdirAll(filepath.Dir(path), 0700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, []byte(`{"hooks":{"Stop":[{"hooks":[{"command":"dev hook agent-response"}]}]}}`), 0600); err != nil {
			t.Fatal(err)
		}
	}
	for _, tc := range []struct {
		args []string
		want int
	}{
		{[]string{"hook", "status", "--project-root", root, "--client", "claude"}, 1},
		{[]string{"hook", "status", "--project-root", root, "--apply"}, 2},
		{[]string{"hook", "disable", "--project-root", root, "--dry-run", "--apply"}, 2},
		{[]string{"hook", "disable", "--project-root", root, "--client", "unknown"}, 1},
		{[]string{"disable", "hooks", "--project-root", root, "--client", "claude"}, 0},
		{[]string{"hook", "status", "--project-root", root, "--client", "claude"}, 0},
		{[]string{"hook", "status", "--project-root", root, "--client", "codex"}, 1},
		{[]string{"hook", "disable", "--project-root", root, "--client", "codex"}, 0},
		{[]string{"hook", "status", "--project-root", root}, 0},
		{[]string{"disable", "hooks", "--project-root"}, 2},
		{[]string{"disable", "hooks", "--prefix", root}, 2},
		{[]string{"hook", "disable", "--project-root", ""}, 2},
		{[]string{"hook", "disable", "--client", "--project-root", root}, 2},
	} {
		out, ro := swapStdout(t)
		errOut, re := swapStderr(t)
		code := dispatch(tc.args)
		re()
		ro()
		if code != tc.want {
			t.Fatalf("%v: exit %d want %d out=%s err=%s", tc.args, code, tc.want, out.String(), errOut.String())
		}
	}
}

func TestHookLegacyFlagsAreInert(t *testing.T) {
	for _, args := range [][]string{{"session-start", "--project-root"}, {"session-end", "--event-json", "{broken"}, {"--client", "claude", "session-end"}, {"post-tool-use", "--defer-batch", "--future-flag", "value"}} {
		if code := runHook(args); code != 0 {
			t.Fatalf("%v: exit %d", args, code)
		}
	}
}
