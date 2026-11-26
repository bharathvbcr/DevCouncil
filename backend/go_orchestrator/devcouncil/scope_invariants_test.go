package devcouncil_test

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/policy"
)

// hardDenial is a rule the gate applies before it reads the plan; no plan entry
// can authorise a path it refuses.
func hardDenial(r policy.RuleID) bool {
	switch r {
	case policy.RuleSecretPath, policy.RuleRestrictedPath, policy.RuleMalformedPath, policy.RuleOutsideRoot:
		return true
	}
	return false
}

// A glob plan entry must not make verify or get_diff call planned a path the
// gate refuses by a hard rule: secret, restricted, tool state, prompt input,
// malformed spelling, or the on-disk alias of one. The matrix is the attack
// probe's, plus a hard link to a root prompt input.
func TestNoGlobPlansWhatTheGateHardDenies(t *testing.T) {
	root := t.TempDir()
	if err := os.WriteFile(filepath.Join(root, "CLAUDE.md"), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.Link(filepath.Join(root, "CLAUDE.md"), filepath.Join(root, "notes.txt")); err != nil {
		t.Fatal(err)
	}
	glob := []dc.PlannedFile{{Path: "*", AllowedChange: dc.ChangeModify}}
	task := &dc.Task{ID: "T", PlannedFiles: glob}
	paths := []string{
		".env", ".ENV", ".env.local", "deploy/server.pem", "secrets\\k", ".git/config", ".GIT/config", ".devcouncil/state.sqlite", ".devmap/repo_map.json",
		"CLAUDE.md", "AGENTS.md", "plugins/x/mcp.json", ".mcp.json", "dir/.mcp.json", ".claude/skills/x/SKILL.md", ".claude/settings.json",
		".git /config", "a.", "a ", "a\nb.go", "a\x00b", "src/../.env", ".//.env", "../x", "notes.txt",
	}
	gate := policy.FileGate{Root: root, HardRules: true}
	denied := 0
	for _, p := range paths {
		d := gate.EvaluateFileChange(p, task, dc.OpModify, false)
		if !d.Blocked() || !hardDenial(d.Rule) {
			t.Logf("%q is not a hard denial here (blocked=%v rule=%s); not asserted", p, d.Blocked(), d.Rule)
			continue
		}
		denied++
		if editable, _ := policy.PlannedScopeIn(root, p, glob); editable {
			t.Errorf("%q: gate hard-denies (%s) but PlannedScopeIn says editable", p, d.Rule)
		}
		for _, gap := range verify.DetectOrphanDiffGapsIn(root, "T", glob, []string{p}, nil) {
			if !gap.Blocking {
				t.Errorf("%q: orphan_diff is advisory for a hard-denied path", p)
			}
		}
		// Tool state is left out of the changed set by verification altogether.
		if len(verify.DetectOrphanDiffGapsIn(root, "T", glob, []string{p}, nil)) == 0 && !policy.IsToolState(p) {
			t.Errorf("%q: gate hard-denies (%s) but verify raises no orphan_diff under a glob plan", p, d.Rule)
		}
	}
	if denied < 15 {
		t.Fatalf("only %d paths were hard denials; the matrix no longer exercises the invariant", denied)
	}
}

// read_only wins on the write gate and in verify, in either order and spelling,
// and would_block_verification agrees with verify wherever the write is allowed.
func TestGateAndVerifyAgreeThatReadOnlyWinsOverlap(t *testing.T) {
	mod := func(p string) dc.PlannedFile { return dc.PlannedFile{Path: p, AllowedChange: dc.ChangeModify} }
	ro := func(p string) dc.PlannedFile { return dc.PlannedFile{Path: p, AllowedChange: dc.ChangeReadOnly} }
	pairs := [][2]dc.PlannedFile{
		{mod("src/a.go"), ro("src/a.go")},
		{mod("src/a.go"), ro("src/*.go")},
		{mod("src/a.go"), ro("./src/a.go")},
		{mod("src/*"), ro("src/a.go")},
	}
	for _, pair := range pairs {
		for _, planned := range [][]dc.PlannedFile{{pair[0], pair[1]}, {pair[1], pair[0]}} {
			root := t.TempDir()
			task := &dc.Task{ID: "T", PlannedFiles: planned}
			d := policy.FileGate{Root: root, HardRules: true}.EvaluateFileChange("src/a.go", task, dc.OpModify, false)
			_, readOnly := policy.PlannedScopeIn(root, "src/a.go", planned)
			if !readOnly || !d.Blocked() || d.Rule != policy.RuleReadOnly {
				t.Errorf("%+v: gate blocked=%v rule=%s, verify read_only=%v; want both read_only", planned, d.Blocked(), d.Rule, readOnly)
			}
			gaps := verify.DetectOrphanDiffGapsIn(root, "T", planned, []string{"src/a.go"}, nil)
			if len(gaps) != 1 || gaps[0].GapType != "oracle_tampering" {
				t.Errorf("%+v: verify gaps %+v, want one oracle_tampering", planned, gaps)
			}
		}
	}
}

// A task built directly (not through the store) with a "./" forbidden entry is
// still refused: the gate normalises the list itself.
func TestGateNormalisesForbiddenChangesItself(t *testing.T) {
	task := &dc.Task{ID: "T", PlannedFiles: []dc.PlannedFile{{Path: "*", AllowedChange: dc.ChangeModify}},
		ForbiddenChanges: []string{"./secret/*", ".\\locked\\x"}}
	g := policy.FileGate{Root: t.TempDir(), HardRules: true}
	for _, p := range []string{"secret/x", "secret/a/b", "locked/x"} {
		if d := g.EvaluateFileChange(p, task, dc.OpModify, false); !d.Blocked() || d.Rule != policy.RuleForbiddenChange {
			t.Errorf("%s: blocked=%v rule=%s, want %s", p, d.Blocked(), d.Rule, policy.RuleForbiddenChange)
		}
	}
	if d := g.EvaluateFileChange("src/a.go", task, dc.OpModify, false); d.Blocked() {
		t.Errorf("src/a.go is planned and not forbidden: %+v", d)
	}
}

// A file literally named "src\a.go" is, to the gate (NormalizeRepoPath reads a
// backslash as a separator) and to verify alike, the planned src/a.go. On POSIX
// the two names are different files; both surfaces fold them the same way, so
// they agree, and the safe direction holds: the fold can only cover a path a
// plan entry names once spelled with slashes.
func TestBackslashChangedPathIsReadTheSameByGateAndVerify(t *testing.T) {
	root := t.TempDir()
	planned := []dc.PlannedFile{{Path: "src/a.go", AllowedChange: dc.ChangeModify}}
	task := &dc.Task{ID: "T", PlannedFiles: planned}
	d := policy.FileGate{Root: root, HardRules: true}.EvaluateFileChange("src\\a.go", task, dc.OpModify, false)
	gaps := verify.DetectOrphanDiffGapsIn(root, "T", planned, []string{"src\\a.go"}, nil)
	if d.Blocked() || policy.WouldBlockVerificationIn(root, d, task) || len(gaps) != 0 {
		t.Errorf("gate blocked=%v would_block=%v verify gaps=%+v; want all three to read src\\a.go as src/a.go", d.Blocked(), policy.WouldBlockVerificationIn(root, d, task), gaps)
	}
	other := verify.DetectOrphanDiffGapsIn(root, "T", planned, []string{"src\\b.go"}, nil)
	if len(other) != 1 || !other[0].Blocking {
		t.Errorf("src\\b.go is not planned: %+v", other)
	}
}
