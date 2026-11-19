package policy

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
)

// hardLinkRepo lays out a repository whose protected files each have a second
// name under src/, the shape ft-84b49733 item 1 reports: a plan of src/* covers
// the link by text, and the write lands on the protected inode.
func hardLinkRepo(t *testing.T) string {
	t.Helper()
	root := t.TempDir()
	files := map[string]string{
		".env":                  "S=1\n",
		"secrets/k":             "k\n",
		".claude/settings.json": "{}\n",
		"docs/AGENTS.md":        "rules\n",
		"src/a.go":              "package a\n",
		"src/x.go":              "package x\n",
	}
	for rel, body := range files {
		full := filepath.Join(root, filepath.FromSlash(rel))
		if err := os.MkdirAll(filepath.Dir(full), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(full, []byte(body), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	for link, target := range map[string]string{
		"src/hardlink":    ".env",
		"src/hl_secrets":  "secrets/k",
		"src/hl_settings": ".claude/settings.json",
		"src/hl_agents":   "docs/AGENTS.md",
		"src/y.go":        "src/x.go",
	} {
		if err := os.Link(filepath.Join(root, target), filepath.Join(root, link)); err != nil {
			t.Skipf("this filesystem does not support hard links: %v", err)
		}
	}
	return root
}

func TestGateDeniesAHardLinkToASecretOrRestrictedFile(t *testing.T) {
	root := hardLinkRepo(t)
	planned := []dc.PlannedFile{modify("src/*")}
	task := &dc.Task{ID: "TASK-1", PlannedFiles: planned}
	g := FileGate{Root: root, HardRules: true}
	for _, tc := range []struct {
		path string
		rule RuleID
	}{
		{"src/hardlink", RuleSecretPath},
		{"src/hl_secrets", RuleSecretPath},
		{"src/hl_settings", RuleRestrictedPath},
		{"src/hl_agents", RuleRestrictedPath},
	} {
		d := g.EvaluateFileChange(tc.path, task, dc.OpModify, false)
		if !d.Blocked() || d.Rule != tc.rule {
			t.Errorf("%s: gate said %s/%q (%s), want a %s denial", tc.path, d.Action, d.Rule, d.Reason, tc.rule)
		}
		if !HardDenied(root, tc.path) {
			t.Errorf("%s: gate refuses a hard link to a protected file but HardDenied does not", tc.path)
		}
		if editable, _ := PlannedScopeIn(root, tc.path, planned); editable {
			t.Errorf("%s: a glob plans a hard link to a protected file", tc.path)
		}
	}
}

// The rung identifies the inode, so it must not over-refuse: a hard-linked pair
// of ordinary files, and the ordinary file next to them, stay writable.
func TestHardLinkRungLeavesOrdinaryLinkedFilesWritable(t *testing.T) {
	root := hardLinkRepo(t)
	task := &dc.Task{ID: "TASK-1", PlannedFiles: []dc.PlannedFile{modify("src/*")}}
	g := FileGate{Root: root, HardRules: true}
	for _, path := range []string{"src/x.go", "src/y.go", "src/a.go"} {
		if d := g.EvaluateFileChange(path, task, dc.OpModify, false); d.Blocked() {
			t.Errorf("%s was refused: %s/%q %s", path, d.Action, d.Rule, d.Reason)
		}
		if HardDenied(root, path) {
			t.Errorf("%s: HardDenied is true for an ordinary file", path)
		}
	}
}

// internal is the host's own write: it is exempt from the restricted rung and
// stays exempt for a hard link to a restricted file, but never for a secret.
func TestHardLinkRungKeepsTheInternalExemptionForRestrictedOnly(t *testing.T) {
	root := hardLinkRepo(t)
	task := &dc.Task{ID: "TASK-1", PlannedFiles: []dc.PlannedFile{modify("src/*")}}
	g := FileGate{Root: root, HardRules: true}
	if d := g.EvaluateFileChange("src/hl_settings", task, dc.OpModify, true); d.Rule == RuleRestrictedPath {
		t.Errorf("internal write to a link of a restricted file was refused as restricted: %s", d.Reason)
	}
	if d := g.EvaluateFileChange("src/hardlink", task, dc.OpModify, true); d.Rule != RuleSecretPath {
		t.Errorf("internal write to a link of a secret: %s/%q, want %s", d.Action, d.Rule, RuleSecretPath)
	}
}

// A search that cannot finish is not a clean pass: with the entry budget spent
// before the repository was read, a file with other names is refused and the
// reason says the search was cut short.
func TestHardLinkRungRefusesWhenTheSearchIsCutShort(t *testing.T) {
	root := hardLinkRepo(t)
	old := hardLinkWalkCap
	hardLinkWalkCap = 1
	t.Cleanup(func() { hardLinkWalkCap = old })
	task := &dc.Task{ID: "TASK-1", PlannedFiles: []dc.PlannedFile{modify("src/*")}}
	g := FileGate{Root: root, HardRules: true}
	d := g.EvaluateFileChange("src/x.go", task, dc.OpModify, false)
	if !d.Blocked() || !strings.Contains(d.Reason, "could not") {
		t.Fatalf("a cut-short search was judged %s/%q (%s), want a refusal that says it could not finish", d.Action, d.Rule, d.Reason)
	}
	if !HardDenied(root, "src/x.go") {
		t.Error("HardDenied reads a cut-short search as a pass")
	}
	// A file with one name needs no search at all.
	if d := g.EvaluateFileChange("src/a.go", task, dc.OpModify, false); d.Blocked() {
		t.Errorf("a file with a single link was refused: %s", d.Reason)
	}
}

// With no root the on-disk rungs cannot run: HardDenied answers from the text
// alone, which is a lower bound, and the judge says so.
func TestScopeJudgeSaysWhetherTheOnDiskRungsRan(t *testing.T) {
	root := hardLinkRepo(t)
	planned := []dc.PlannedFile{modify("src/*")}
	if got := NewScopeJudge("").Judge("src/hardlink", planned); !got.Editable || got.OnDiskChecked {
		t.Errorf("no-root judge: %+v, want editable (text only) with OnDiskChecked=false", got)
	}
	if got := NewScopeJudge(root).Judge("src/hardlink", planned); got.Editable || !got.OnDiskChecked {
		t.Errorf("rooted judge: %+v, want not editable with OnDiskChecked=true", got)
	}
}

// A symlink to a secret reads as planned without a root; PlansPathIn and
// WouldBlockVerificationIn take the root, so the alias rungs speak.
func TestPlansPathInSeesAnAliasThatPlansPathCannot(t *testing.T) {
	root := t.TempDir()
	if err := os.WriteFile(filepath.Join(root, ".env"), []byte("S=1\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(filepath.Join(root, "src"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink("../.env", filepath.Join(root, "src", "x")); err != nil {
		t.Skipf("symlinks unavailable: %v", err)
	}
	task := &dc.Task{ID: "TASK-1", PlannedFiles: []dc.PlannedFile{modify("src/*")}}
	if !PlansPath(task, "src/x") {
		t.Fatal("the text-only answer changed: src/x is planned by src/* without a root")
	}
	if PlansPathIn(root, task, "src/x") {
		t.Error("PlansPathIn reads a symlink to .env as planned")
	}
	allowed := Decision{Action: Allow, Target: "src/x"}
	if !WouldBlockVerificationIn(root, allowed, task) {
		t.Error("WouldBlockVerificationIn says verification passes a symlink to .env")
	}
}

func TestHardDeniedEvaluationsArePerPathNotPerEntry(t *testing.T) {
	root := hardLinkRepo(t)
	var planned []dc.PlannedFile
	for i := 0; i < 100; i++ {
		planned = append(planned, modify("pkg"+string(rune('a'+i%26))+"/*.go"))
	}
	paths := []string{"src/a.go", "src/x.go", "pkga/f.go", "pkgb/f.go", "pkgc/f.go"}
	judge := NewScopeJudge(root)
	before := HardDeniedEvaluations()
	for _, entry := range planned {
		for _, p := range paths {
			judge.Scope(p, []dc.PlannedFile{entry})
		}
	}
	if got := HardDeniedEvaluations() - before; got > int64(len(paths)) {
		t.Errorf("%d HardDenied evaluations for %d entries x %d paths, want at most one per path (%d)",
			got, len(planned), len(paths), len(paths))
	}
}
