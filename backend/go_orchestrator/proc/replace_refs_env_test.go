//go:build unix

package proc

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

// The replace-ref guard (core.useReplaceRefs=false at the git seam, see
// replace_refs_test.go) must hold against the environment as well as the
// repository: a replace ref under another base named in GIT_REPLACE_REF_BASE,
// and config handed to git through GIT_CONFIG_COUNT, GIT_CONFIG_PARAMETERS,
// GIT_CONFIG_GLOBAL and GIT_CONFIG_SYSTEM that asks for replace refs back on.
// These were round-3 attacks on the range reader (TestR2ReplaceRefBaseEnv... and
// TestR2EnvironmentConfigCannotTurnReplaceRefsBackOn); the reader is replaced by
// a degrade, so they drive the seam the reader stood on, session.output, as
// replace_refs_test.go does.

// replacedCommit makes a repository whose HEAD commit adds a credential and has
// a clean twin, and returns the root and the commit's id. The replacement is
// installed by install, so a case can choose the ref namespace.
func replacedCommit(t *testing.T, install func(root, bad, clean string)) (root, bad string) {
	t.Helper()
	root = gitRepo(t)
	gitRun(t, root, "add", "a.txt")
	gitRun(t, root, "-c", "user.email=a@example.test", "-c", "user.name=a", "commit", "-qm", "adds the secret")
	bad = strings.TrimSpace(gitOutput(t, root, "rev-parse", "HEAD"))
	tree := strings.TrimSpace(gitOutput(t, root, "rev-parse", bad+"^^{tree}"))
	parent := strings.TrimSpace(gitOutput(t, root, "rev-parse", bad+"^"))
	clean := strings.TrimSpace(gitOutput(t, root, "-c", "user.email=a@example.test", "-c", "user.name=a", "commit-tree", tree, "-p", parent, "-m", "clean twin"))
	install(root, bad, clean)
	return root, bad
}

// sessionReadsTheCommitThatWasMade requires every read through a session to see
// the commit as it was made, not its twin.
func sessionReadsTheCommitThatWasMade(t *testing.T, root, bad string) {
	t.Helper()
	session, err := NewGitSession(root)
	if err != nil {
		t.Fatal(err)
	}
	for _, args := range [][]string{
		{"log", "--format=%s", "-1", bad},
		{"show", "--no-patch", "--format=%s", bad},
		{"cat-file", "commit", bad},
	} {
		got, err := session.output(t.Context(), 1<<16, args...)
		if err != nil {
			t.Fatalf("git %v: %v", args, err)
		}
		if !strings.Contains(string(got), "adds the secret") || strings.Contains(string(got), "clean twin") {
			t.Fatalf("git %v read the replacement: %q", args, got)
		}
	}
}

// A replace ref kept under another base, with the base named in the
// environment, is one git honours without any config.
func TestReplaceRefBaseEnvDoesNotRewriteTheRead(t *testing.T) {
	root, bad := replacedCommit(t, func(root, bad, clean string) {
		gitRun(t, root, "update-ref", "refs/replace2/"+bad, clean)
	})
	t.Setenv("GIT_REPLACE_REF_BASE", "refs/replace2/")
	control := exec.Command("git", "log", "--format=%s", "-1", bad)
	control.Dir = root
	control.Env = append(os.Environ(), "GIT_CONFIG_NOSYSTEM=1", "GIT_CONFIG_GLOBAL="+os.DevNull)
	if out, err := control.Output(); err != nil || strings.TrimSpace(string(out)) != "clean twin" {
		t.Fatalf("the control read does not honour the replace ref under the base, so this test proves nothing: %q, %v", out, err)
	}
	sessionReadsTheCommitThatWasMade(t, root, bad)
}

// Hostile environment config asks for the replace refs back on.
func TestEnvironmentConfigCannotTurnReplaceRefsBackOn(t *testing.T) {
	cases := map[string]func(t *testing.T, dir string){
		"GIT_CONFIG_COUNT": func(t *testing.T, _ string) {
			t.Setenv("GIT_CONFIG_COUNT", "1")
			t.Setenv("GIT_CONFIG_KEY_0", "core.useReplaceRefs")
			t.Setenv("GIT_CONFIG_VALUE_0", "true")
		},
		"GIT_CONFIG_PARAMETERS": func(t *testing.T, _ string) {
			t.Setenv("GIT_CONFIG_PARAMETERS", "'core.usereplacerefs=true'")
		},
		"GIT_CONFIG_GLOBAL": func(t *testing.T, dir string) {
			f := filepath.Join(dir, "global")
			if err := os.WriteFile(f, []byte("[core]\n\tuseReplaceRefs = true\n"), 0o644); err != nil {
				t.Fatal(err)
			}
			t.Setenv("GIT_CONFIG_GLOBAL", f)
		},
		"GIT_CONFIG_SYSTEM": func(t *testing.T, dir string) {
			f := filepath.Join(dir, "system")
			if err := os.WriteFile(f, []byte("[core]\n\tuseReplaceRefs = true\n"), 0o644); err != nil {
				t.Fatal(err)
			}
			t.Setenv("GIT_CONFIG_SYSTEM", f)
		},
	}
	for name, set := range cases {
		t.Run(name, func(t *testing.T) {
			root, bad := replacedCommit(t, func(root, bad, clean string) {
				gitRun(t, root, "replace", bad, clean)
			})
			set(t, t.TempDir())
			sessionReadsTheCommitThatWasMade(t, root, bad)
		})
	}
}
