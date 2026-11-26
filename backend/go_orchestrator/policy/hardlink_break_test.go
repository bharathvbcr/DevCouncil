package policy

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
)

// A hard link to a secret that lives outside the repository. The walk only reads
// root, so the other name is never seen; the link count says it exists.
func TestHLAHardLinkToASecretOutsideTheRepo(t *testing.T) {
	outside := t.TempDir()
	secret := hlaWrite(t, outside, "id_rsa", "KEY")
	root := t.TempDir()
	hlaWrite(t, root, "src/seed.go", "package s")
	hlaLink(t, secret, filepath.Join(root, "src", "k"))
	g, task := hlaGate(root)
	if d := g.EvaluateFileChange("src/k", task, dc.OpModify, false); !d.Blocked() {
		t.Errorf("hard link to an out-of-repo id_rsa: %s (%s), want denied", d.Action, d.Reason)
	}
}

// The same, for a link to a hook of the main repository when root is a linked
// worktree (its .git is a file; the hooks live outside root).
func TestHLAHardLinkToSharedGitHooksFromAWorktree(t *testing.T) {
	main := t.TempDir()
	hook := hlaWrite(t, main, ".git/hooks/pre-commit", "#!/bin/sh\n")
	wt := t.TempDir()
	hlaWrite(t, wt, ".git", "gitdir: "+filepath.Join(main, ".git", "worktrees", "w")+"\n")
	hlaWrite(t, wt, "src/seed.go", "package s")
	hlaLink(t, hook, filepath.Join(wt, "src", "h"))
	g, task := hlaGate(wt)
	if d := g.EvaluateFileChange("src/h", task, dc.OpModify, false); !d.Blocked() {
		t.Errorf("hard link to the shared hook: %s (%s), want denied", d.Action, d.Reason)
	}
}

// Root given as a symbolic link to the repository. WalkDir does not follow a
// symlink root, so it reads one entry and finds nothing.
func TestHLAHardLinkRungWithASymlinkedRoot(t *testing.T) {
	real := t.TempDir()
	hlaWrite(t, real, ".env", "S=1")
	hlaWrite(t, real, "src/seed.go", "package s")
	hlaLink(t, filepath.Join(real, ".env"), filepath.Join(real, "src", "hl"))
	alias := filepath.Join(t.TempDir(), "repo")
	if err := os.Symlink(real, alias); err != nil {
		t.Skipf("symlinks unavailable: %v", err)
	}
	g, task := hlaGate(alias)
	if d := g.EvaluateFileChange("src/hl", task, dc.OpModify, false); !d.Blocked() {
		t.Errorf("symlinked root, link to .env: %s (%s), want denied", d.Action, d.Reason)
	}
}

// ".." is not resolved in a forbidden_changes entry, so the entry forbade
// nothing.
func TestAdvForbiddenEntryWithDotDot(t *testing.T) {
	g := FileGate{Root: t.TempDir(), HardRules: true}
	task := &dc.Task{ID: "T", PlannedFiles: []dc.PlannedFile{modify("lib/*")}, ForbiddenChanges: []string{"src/../lib/x.go"}}
	if d := g.EvaluateFileChange("lib/x.go", task, dc.OpModify, false); !d.Blocked() {
		t.Errorf("forbidden src/../lib/x.go does not forbid lib/x.go: %s", d.Action)
	}
}

// A forbidden entry ending in "/" or "/." names a directory; it used to match
// nothing. It forbids the directory and everything under it.
func TestForbiddenDirectoryEntriesForbidTheTree(t *testing.T) {
	g := FileGate{Root: t.TempDir(), HardRules: true}
	for _, entry := range []string{"secret/x/", "secret/x/.", "./secret/x/", "secret//x//"} {
		task := &dc.Task{ID: "T", PlannedFiles: []dc.PlannedFile{modify("*")}, ForbiddenChanges: []string{entry}}
		for _, p := range []string{"secret/x", "secret/x/a.txt", "secret/x/deep/b.txt"} {
			if d := g.EvaluateFileChange(p, task, dc.OpModify, false); d.Rule != RuleForbiddenChange {
				t.Errorf("forbidden %q, path %q: %s/%q, want %s", entry, p, d.Action, d.Rule, RuleForbiddenChange)
			}
		}
		if d := g.EvaluateFileChange("secret/xy", task, dc.OpModify, false); d.Rule == RuleForbiddenChange {
			t.Errorf("forbidden %q also forbids the sibling secret/xy", entry)
		}
	}
}

// A hard link whose other names are not all visible from the root is refused, with
// a reason that says why; an in-repo-only link whose names are all visible stays
// writable (TestHardLinkRungLeavesOrdinaryLinkedFilesWritable).
func TestHardLinkWithInvisibleNamesIsRefusedWithAReason(t *testing.T) {
	outside := t.TempDir()
	other := hlaWrite(t, outside, "cache/pkg.js", "x")
	root := t.TempDir()
	hlaWrite(t, root, "src/seed.go", "package s")
	hlaLink(t, other, filepath.Join(root, "src", "pkg.js"))
	g, task := hlaGate(root)
	d := g.EvaluateFileChange("src/pkg.js", task, dc.OpModify, false)
	if !d.Blocked() || !strings.Contains(d.Reason, "hard link") || !strings.Contains(d.Reason, "outside") {
		t.Errorf("link with a name outside the root: %s/%q (%s), want a refusal naming that", d.Action, d.Rule, d.Reason)
	}
}
