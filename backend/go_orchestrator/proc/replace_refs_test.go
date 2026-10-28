//go:build unix

package proc

import (
	"os"
	"os/exec"
	"strings"
	"testing"
)

// A `git replace` ref rewrites history at read time, and the repository under
// analysis can carry one: the commit that adds a credential, replaced by a clean
// twin, reads as the twin. A read through the session must see the commit that
// was made.
func TestAReplaceRefDoesNotRewriteWhatASessionReadSees(t *testing.T) {
	root := gitRepo(t)
	gitRun(t, root, "add", "a.txt")
	gitRun(t, root, "-c", "user.email=a@example.test", "-c", "user.name=a", "commit", "-qm", "adds the secret")
	bad := strings.TrimSpace(gitOutput(t, root, "rev-parse", "HEAD"))
	tree := strings.TrimSpace(gitOutput(t, root, "rev-parse", bad+"^^{tree}"))
	parent := strings.TrimSpace(gitOutput(t, root, "rev-parse", bad+"^"))
	clean := strings.TrimSpace(gitOutput(t, root, "-c", "user.email=a@example.test", "-c", "user.name=a", "commit-tree", tree, "-p", parent, "-m", "clean twin"))
	gitRun(t, root, "replace", bad, clean)

	control := exec.Command("git", "log", "--format=%s", "-1", bad)
	control.Dir = root
	control.Env = append(os.Environ(), "GIT_CONFIG_NOSYSTEM=1", "GIT_CONFIG_GLOBAL="+os.DevNull)
	out, err := control.Output()
	if err != nil || strings.TrimSpace(string(out)) != "clean twin" {
		t.Fatalf("the control read does not honour the replace ref, so this test proves nothing: %q, %v", out, err)
	}

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
