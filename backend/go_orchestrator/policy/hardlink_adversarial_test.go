package policy

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
)

func hlaWrite(t *testing.T, root, rel, body string) string {
	t.Helper()
	full := filepath.Join(root, filepath.FromSlash(rel))
	if err := os.MkdirAll(filepath.Dir(full), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(full, []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
	return full
}

func hlaLink(t *testing.T, oldname, newname string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(newname), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.Link(oldname, newname); err != nil {
		t.Skipf("hard links unavailable: %v", err)
	}
}

func hlaGate(root string) (FileGate, *dc.Task) {
	return FileGate{Root: root, HardRules: true},
		&dc.Task{ID: "TASK-1", PlannedFiles: []dc.PlannedFile{modify("src/*")}}
}

// Pinned: other names of a protected inode that live inside the repository are
// found whatever directory they sit in, .git and tool state included, and a
// case-variant directory name is folded.
func TestHLAHardLinkInRepoProtectedNamesAreAllFound(t *testing.T) {
	root := t.TempDir()
	for rel, body := range map[string]string{
		".git/hooks/pre-commit": "#!/bin/sh\n",
		".git/config":           "[core]\n",
		".devcouncil/tasks.db":  "db",
		"SECRETS/key.txt":       "k",
		"deep/a/b/c/.env.local": "S=1",
		"keys/server.pem":       "pem",
		"AGENTS.md":             "rules",
		"src/seed.go":           "package s",
	} {
		hlaWrite(t, root, rel, body)
	}
	cases := map[string]struct {
		target string
		rule   RuleID
	}{
		"src/h_hook":   {".git/hooks/pre-commit", RuleRestrictedPath},
		"src/h_gitcfg": {".git/config", RuleRestrictedPath},
		"src/h_state":  {".devcouncil/tasks.db", RuleRestrictedPath},
		"src/h_upper":  {"SECRETS/key.txt", RuleSecretPath},
		"src/h_deep":   {"deep/a/b/c/.env.local", RuleSecretPath},
		"src/h_pem":    {"keys/server.pem", RuleSecretPath},
		"src/h_agents": {"AGENTS.md", RuleRestrictedPath},
	}
	g, task := hlaGate(root)
	for link, tc := range cases {
		hlaLink(t, filepath.Join(root, filepath.FromSlash(tc.target)), filepath.Join(root, filepath.FromSlash(link)))
		d := g.EvaluateFileChange(link, task, dc.OpModify, false)
		if !d.Blocked() || d.Rule != tc.rule {
			t.Errorf("%s -> %s: %s/%q (%s), want %s", link, tc.target, d.Action, d.Rule, d.Reason, tc.rule)
		}
		if !HardDenied(root, link) {
			t.Errorf("%s: HardDenied false", link)
		}
	}
}

// Pinned: a symlink to a hard link of a secret, and the link spelled through a
// case variant, are both refused.
func TestHLASymlinkToHardLinkAndCaseVariantSpelling(t *testing.T) {
	root := t.TempDir()
	hlaWrite(t, root, ".env", "S=1")
	hlaLink(t, filepath.Join(root, ".env"), filepath.Join(root, "src", "hl"))
	if err := os.Symlink("hl", filepath.Join(root, "src", "sym")); err != nil {
		t.Skipf("symlinks unavailable: %v", err)
	}
	g, task := hlaGate(root)
	for _, p := range []string{"src/sym", "src/hl", "./src//hl", "src/../src/hl"} {
		if d := g.EvaluateFileChange(p, task, dc.OpModify, false); !d.Blocked() {
			t.Errorf("%q: %s/%q (%s), want denied", p, d.Action, d.Rule, d.Reason)
		}
	}
}

// Pinned: rename-swap. A file that was a singly-linked ordinary file (allowed)
// and is replaced by a link to the secret is judged on its new link count; the
// decision is made at call time, so a link made after an earlier allow is only
// caught by the next call (TOCTOU, documented: the gate is not a lock).
func TestHLARenameSwapAndLateLinkAreJudgedAtCallTime(t *testing.T) {
	root := t.TempDir()
	hlaWrite(t, root, ".env", "S=1")
	hlaWrite(t, root, "src/a.go", "package a")
	g, task := hlaGate(root)
	if d := g.EvaluateFileChange("src/a.go", task, dc.OpModify, false); d.Blocked() {
		t.Fatalf("ordinary file refused: %s", d.Reason)
	}
	late := filepath.Join(root, "src", "late")
	if d := g.EvaluateFileChange("src/late", task, dc.OpModify, false); d.Blocked() {
		t.Fatalf("name not yet on disk refused: %s", d.Reason)
	}
	hlaLink(t, filepath.Join(root, ".env"), late) // after the decision
	if d := g.EvaluateFileChange("src/late", task, dc.OpModify, false); !d.Blocked() || d.Rule != RuleSecretPath {
		t.Errorf("re-judging the late link: %s/%q", d.Action, d.Rule)
	}
	if err := os.Remove(filepath.Join(root, "src", "a.go")); err != nil {
		t.Fatal(err)
	}
	hlaLink(t, filepath.Join(root, ".env"), filepath.Join(root, "src", "a.go"))
	if d := g.EvaluateFileChange("src/a.go", task, dc.OpModify, false); !d.Blocked() || d.Rule != RuleSecretPath {
		t.Errorf("rename-swap: %s/%q", d.Action, d.Rule)
	}
}

// Pinned: once the secret's own name is gone the inode has one link and the rung
// has nothing to say; the file is an ordinary one by name. Documented limit:
// the rung protects names, not content.
func TestHLALinkSurvivingTheSecretsDeletionIsOrdinary(t *testing.T) {
	root := t.TempDir()
	env := hlaWrite(t, root, ".env", "S=1")
	hlaLink(t, env, filepath.Join(root, "src", "keep"))
	if err := os.Remove(env); err != nil {
		t.Fatal(err)
	}
	g, task := hlaGate(root)
	if d := g.EvaluateFileChange("src/keep", task, dc.OpModify, false); d.Blocked() {
		t.Errorf("single-link survivor refused: %s", d.Reason)
	}
}

// Pinned: a walk that cannot read a directory refuses a multi-linked file whose
// other names it did not find, and still names the secret when it did find it.
func TestHLAUnreadableDirectoryInTheWalk(t *testing.T) {
	if os.Geteuid() == 0 {
		t.Skip("root reads every directory")
	}
	root := t.TempDir()
	hlaWrite(t, root, "src/x.go", "package x")
	hlaLink(t, filepath.Join(root, "src", "x.go"), filepath.Join(root, "src", "y.go"))
	locked := filepath.Join(root, "zz_locked")
	if err := os.MkdirAll(locked, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.Chmod(locked, 0); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.Chmod(locked, 0o755) })
	g, task := hlaGate(root)
	d := g.EvaluateFileChange("src/x.go", task, dc.OpModify, false)
	if !d.Blocked() || !strings.Contains(d.Reason, "could not") {
		t.Errorf("unreadable directory: %s/%q (%s), want a refusal that says the search could not finish", d.Action, d.Rule, d.Reason)
	}
	// the secret's name is found before the unreadable directory: it speaks.
	hlaWrite(t, root, ".env", "S=1")
	hlaLink(t, filepath.Join(root, ".env"), filepath.Join(root, "src", "e"))
	if d := g.EvaluateFileChange("src/e", task, dc.OpModify, false); d.Rule != RuleSecretPath {
		t.Errorf("secret found with an unreadable sibling: %s/%q", d.Action, d.Rule)
	}
}

// Pinned: the cap counts entries read, so a repository of exactly cap entries
// still finishes and one more refuses.
func TestHLAEntryCapBoundary(t *testing.T) {
	root := t.TempDir()
	hlaWrite(t, root, "src/x.go", "package x")
	hlaLink(t, filepath.Join(root, "src", "x.go"), filepath.Join(root, "src", "y.go"))
	// entries: root, src, x.go, y.go
	old := hardLinkWalkCap
	t.Cleanup(func() { hardLinkWalkCap = old })
	g, task := hlaGate(root)
	hardLinkWalkCap = 4
	if d := g.EvaluateFileChange("src/x.go", task, dc.OpModify, false); d.Blocked() {
		t.Errorf("cap == entries refused: %s", d.Reason)
	}
	hardLinkWalkCap = 3
	if d := g.EvaluateFileChange("src/x.go", task, dc.OpModify, false); !d.Blocked() {
		t.Errorf("cap < entries allowed")
	}
}
