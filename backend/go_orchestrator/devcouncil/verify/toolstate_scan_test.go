package verify_test

import (
	"bytes"
	"context"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
)

// Tool state is secret-scanned.
//
// .devcouncil and .devmap are what DevCouncil's own tools write, and the diff
// verification judges leaves them out: a file the harness wrote is not work the
// agent did. That left a credential written there in no scan at all, though it
// is committed with the rest. Owner decision (2026-10-28): the secret gate
// reads them, and only the secret gate: no stub, scope, coverage or substance
// verdict is made about a file the harness owns.
//
// What is read is what git sees: tracked files, and untracked files that are
// not ignored. THE LIMIT, NAMED: a file git ignores is not read. In this
// repository .devcouncil/ and .devmap/ are ignored, so a secret written to an
// ignored path under them (the operator's secrets file beside secrets.env.example,
// say) stays unscanned by design; scanning ignored state would read the task
// store and the repo map on every verify, and would block on the operator's own
// credentials.

func toolStateKeyLine() string { return attackSecretLine() }

// A tracked tool-state file modified to hold a credential.
func TestToolStateTrackedFileGainingASecretIsFound(t *testing.T) {
	key := exampleAWSKey()
	root := gitRepo(t)
	writeFile(t, root, ".devcouncil/notes.txt", "notes\n")
	attackCommit(t, root, "track a tool-state file")
	writeFile(t, root, ".devcouncil/notes.txt", "notes\n"+toolStateKeyLine())
	o := attackRun(t, root, "TASK-TS-TRACKED", benignChange(t, root))
	requireLooked(t, key, o)
}

// The change is a tool-state file and nothing else: the main diff is empty, so
// the second pass must not be gated on it.
func TestToolStateOnlySecretIsFound(t *testing.T) {
	key := exampleAWSKey()
	root := gitRepo(t)
	writeFile(t, root, ".devmap/notes/keys.txt", toolStateKeyLine())
	o := attackRun(t, root, "TASK-TS-ONLY")
	if o.refused == nil && !o.empty {
		t.Fatalf("the test's premise is a change the main diff does not hold: empty=%v", o.empty)
	}
	requireLooked(t, key, o)
}

// IsToolState folds case, so `.DEVMAP/` is tool state, and the tracked listing
// matches it the same way.
func TestToolStateIsMatchedCaseInsensitively(t *testing.T) {
	key := exampleAWSKey()
	t.Run("untracked", func(t *testing.T) {
		root := gitRepo(t)
		writeFile(t, root, ".DEVMAP/notes/keys.txt", toolStateKeyLine())
		requireLooked(t, key, attackRun(t, root, "TASK-TS-CASE-U", benignChange(t, root)))
	})
	t.Run("tracked", func(t *testing.T) {
		root := gitRepo(t)
		writeFile(t, root, ".Devcouncil/k.txt", "k\n")
		attackCommit(t, root, "track")
		writeFile(t, root, ".Devcouncil/k.txt", "k\n"+toolStateKeyLine())
		requireLooked(t, key, attackRun(t, root, "TASK-TS-CASE-T", benignChange(t, root)))
	})
}

// A tool-state file spelled like a credential file is found, and nothing it
// holds is echoed.
func TestToolStateSecretPathIsFoundAndWithheld(t *testing.T) {
	key := exampleAWSKey()
	root := gitRepo(t)
	writeFile(t, root, ".devcouncil/.env", toolStateKeyLine())
	o := attackRun(t, root, "TASK-TS-ENV", benignChange(t, root))
	requireLooked(t, key, o)
	noKeyAnywhere(t, key, o)
}

// Only the secret gate reads tool state. A placeholder in a file the harness
// wrote is not this task's stub.
func TestToolStateIsReadByTheSecretGateAlone(t *testing.T) {
	root := gitRepo(t)
	writeFile(t, root, ".devmap/gen.rs", "// TODO: regenerate\npub fn f() -> u32 {\n    todo!()\n}\n")
	o := attackRun(t, root, "TASK-TS-STUBS", benignChange(t, root))
	for _, g := range o.gaps {
		if g.GapType == "stub_detected" || g.GapType == "stub_declared" || g.GapType == "assert_free_test" || g.GapType == "skipped_test" {
			t.Fatalf("a %s gap came from tool state: %s", g.GapType, g.Description)
		}
	}
	if o.result.Status != "verified" || len(o.result.DegradedReasons) != 0 {
		t.Fatalf("status=%s degraded=%q types=%v", o.result.Status, o.result.DegradedReasons, advTypes(o.gaps))
	}
}

// A binary file under tool state is scanned too, and named: dcverify reads a
// binary section's bytes from the working tree, for a path it is told is in
// scope, so a credential in a NUL-bearing file is found. The reason says the
// file was not read as text, whether or not a credential was in it.
func TestToolStateBinaryFileIsScannedAndNamed(t *testing.T) {
	key := exampleAWSKey()
	t.Run("untracked, no credential", func(t *testing.T) {
		root := gitRepo(t)
		writeFile(t, root, ".devmap/x.sqlite", "SQLite format 3\x00rows\n")
		o := attackRun(t, root, "TASK-TS-BIN-U", benignChange(t, root))
		requireDegraded(t, key, o, "tool state not text-scanned (binary): .devmap/x.sqlite")
	})
	t.Run("tracked, no credential", func(t *testing.T) {
		root := gitRepo(t)
		writeFile(t, root, ".devmap/y.sqlite", "SQLite format 3\x00one\n")
		attackCommit(t, root, "track a database")
		writeFile(t, root, ".devmap/y.sqlite", "SQLite format 3\x00two\n")
		o := attackRun(t, root, "TASK-TS-BIN-T", benignChange(t, root))
		requireDegraded(t, key, o, "tool state not text-scanned (binary): .devmap/y.sqlite")
	})
	t.Run("untracked, with a credential", func(t *testing.T) {
		root := gitRepo(t)
		writeFile(t, root, ".devmap/x.sqlite", "SQLite format 3\x00"+toolStateKeyLine())
		requireLooked(t, key, attackRun(t, root, "TASK-TS-BIN-UK", benignChange(t, root)))
	})
	t.Run("tracked, with a credential", func(t *testing.T) {
		root := gitRepo(t)
		writeFile(t, root, ".devmap/y.sqlite", "SQLite format 3\x00one\n")
		attackCommit(t, root, "track a database")
		writeFile(t, root, ".devmap/y.sqlite", "SQLite format 3\x00two\n"+toolStateKeyLine())
		requireLooked(t, key, attackRun(t, root, "TASK-TS-BIN-TK", benignChange(t, root)))
	})
	t.Run("a -diff attribute the agent wrote", func(t *testing.T) {
		root := gitRepo(t)
		writeFile(t, root, ".devmap/b.txt", "x\n")
		attackCommit(t, root, "track")
		writeFile(t, root, ".gitattributes", ".devmap/* -diff\n")
		writeFile(t, root, ".devmap/b.txt", "x\n"+toolStateKeyLine())
		requireLooked(t, key, attackRun(t, root, "TASK-TS-BIN-ATTR", benignChange(t, root)))
	})
}

// Planned carries the tool-state paths so a binary section is read, and nothing
// else: a planned tool-state path that is not in the diff, or a tool-state file
// no plan names, plants no orphan or planned-file gap, because only secret
// findings are kept.
func TestToolStatePlannedPathsLeakNoScopeGap(t *testing.T) {
	root := gitRepo(t)
	writeFile(t, root, ".devmap/x.sqlite", "SQLite format 3\x00rows\n")
	writeFile(t, root, ".devcouncil/notes.txt", "notes\n")
	o := attackRun(t, root, "TASK-TS-SCOPE", benignChange(t, root))
	for _, g := range o.gaps {
		if g.GapType == "orphan_diff" || g.GapType == "planned_file_not_changed" || g.GapType == "task_not_implemented" {
			t.Fatalf("a scope gap came from the tool-state pass: %s %s", g.GapType, g.Description)
		}
	}
}

// A secret staged and then reverted or deleted on disk is in the index, and so in
// the next commit, and in no `git diff HEAD`. The index is read too.
func TestToolStateStagedThenRevertedIsFound(t *testing.T) {
	key := exampleAWSKey()
	seed := func(t *testing.T) string {
		root := gitRepo(t)
		writeFile(t, root, ".devcouncil/a.txt", "a\n")
		attackCommit(t, root, "track")
		return root
	}
	t.Run("tracked, staged with the secret, worktree reverted", func(t *testing.T) {
		root := seed(t)
		writeFile(t, root, ".devcouncil/a.txt", "a\n"+toolStateKeyLine())
		runGitIn(t, root, "add", ".devcouncil/a.txt")
		writeFile(t, root, ".devcouncil/a.txt", "a\n")
		requireLooked(t, key, attackRun(t, root, "TASK-TS-IDX-1", benignChange(t, root)))
	})
	t.Run("new file staged with the secret, overwritten clean", func(t *testing.T) {
		root := seed(t)
		writeFile(t, root, ".devcouncil/new.txt", toolStateKeyLine())
		runGitIn(t, root, "add", ".devcouncil/new.txt")
		writeFile(t, root, ".devcouncil/new.txt", "clean\n")
		requireLooked(t, key, attackRun(t, root, "TASK-TS-IDX-2", benignChange(t, root)))
	})
	t.Run("new file staged with the secret, deleted from disk", func(t *testing.T) {
		root := seed(t)
		writeFile(t, root, ".devcouncil/gone.txt", toolStateKeyLine())
		runGitIn(t, root, "add", ".devcouncil/gone.txt")
		if err := os.Remove(filepath.Join(root, ".devcouncil/gone.txt")); err != nil {
			t.Fatal(err)
		}
		requireLooked(t, key, attackRun(t, root, "TASK-TS-IDX-3", benignChange(t, root)))
	})
	t.Run("a binary blob staged, then deleted from disk", func(t *testing.T) {
		root := seed(t)
		writeFile(t, root, ".devmap/blob.bin", "\x00"+toolStateKeyLine())
		runGitIn(t, root, "add", ".devmap/blob.bin")
		if err := os.Remove(filepath.Join(root, ".devmap/blob.bin")); err != nil {
			t.Fatal(err)
		}
		requireLooked(t, key, attackRun(t, root, "TASK-TS-IDX-4", benignChange(t, root)))
	})
	t.Run("a repository with no HEAD, staged then reverted", func(t *testing.T) {
		root := t.TempDir()
		runGitIn(t, root, "init", "-q")
		writeFile(t, root, ".devcouncil/k.txt", toolStateKeyLine())
		runGitIn(t, root, "add", ".devcouncil/k.txt")
		writeFile(t, root, ".devcouncil/k.txt", "clean\n")
		requireLooked(t, key, attackRun(t, root, "TASK-TS-IDX-5"))
	})
}

// Where the index and the worktree differ both versions are scanned; where they
// agree the path is read once.
func TestToolStateIndexAndWorktreeVersionsAreBothScannedAndNotDoubled(t *testing.T) {
	root := gitRepo(t)
	writeFile(t, root, ".devcouncil/a.txt", "a\n")
	writeFile(t, root, ".devcouncil/same.txt", "s\n")
	attackCommit(t, root, "track")
	writeFile(t, root, ".devcouncil/a.txt", "a\nindex version\n")
	runGitIn(t, root, "add", ".devcouncil/a.txt")
	writeFile(t, root, ".devcouncil/a.txt", "a\nworktree version\n")
	writeFile(t, root, ".devcouncil/same.txt", "s\nstaged and identical\n")
	runGitIn(t, root, "add", ".devcouncil/same.txt")
	state, err := verify.CollectGitStateReport(context.Background(), root)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(state.ToolStateDiff, "+worktree version") || strings.Contains(state.ToolStateDiff, "+index version") {
		t.Fatalf("the worktree view is wrong:\n%s", state.ToolStateDiff)
	}
	if !strings.Contains(state.ToolStateIndexDiff, "+index version") || strings.Contains(state.ToolStateIndexDiff, "worktree version") {
		t.Fatalf("the index view is wrong:\n%s", state.ToolStateIndexDiff)
	}
	if strings.Contains(state.ToolStateIndexDiff, "same.txt") {
		t.Fatalf("a path whose index and worktree agree was read twice:\n%s", state.ToolStateIndexDiff)
	}
	if strings.Count(state.ToolStateIndexDiff, "diff --git a/.devcouncil/a.txt") != 1 || strings.Count(state.ToolStateDiff, "diff --git a/.devcouncil/a.txt") != 1 {
		t.Fatalf("a path is named twice in one diff")
	}
}

// A tool-state symlink is not followed, and is named.
func TestToolStateSymlinkIsNotFollowedAndIsNamed(t *testing.T) {
	key := exampleAWSKey()
	root := gitRepo(t)
	outside := filepath.Join(t.TempDir(), "keys.txt")
	if err := os.WriteFile(outside, []byte(toolStateKeyLine()), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(filepath.Join(root, ".devcouncil"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(outside, filepath.Join(root, ".devcouncil", "link")); err != nil {
		t.Fatal(err)
	}
	o := attackRun(t, root, "TASK-TS-LINK", benignChange(t, root))
	requireDegraded(t, key, o, "tool state not secret-scanned: .devcouncil/link (symlink)")
	for _, g := range o.gaps {
		if g.GapType == "security_risk" {
			t.Fatalf("the symlink's target was read: %s", g.Description)
		}
	}
}

// Tool state past the diff cap is not scanned, and says so rather than being
// scanned in part.
func TestToolStatePastTheCapIsNamed(t *testing.T) {
	key := exampleAWSKey()
	root := gitRepo(t)
	writeFile(t, root, ".devmap/big.txt", strings.Repeat("a", (32<<20)+1))
	o := attackRun(t, root, "TASK-TS-CAP", benignChange(t, root))
	requireDegraded(t, key, o, "tool state not secret-scanned: its diff exceeds 33554432 bytes")
}

// THE LIMIT: git ignores .devcouncil/ here, as it does in this repository, and a
// credential in an ignored file is not read. This test holds the limit as it is
// so that moving it is a decision and not a drift.
func TestToolStateIgnoredByGitIsOutOfScope(t *testing.T) {
	key := exampleAWSKey()
	root := gitRepo(t)
	writeFile(t, root, ".gitignore", ".devcouncil/\n.devmap/\n")
	attackCommit(t, root, "ignore tool state")
	writeFile(t, root, ".devcouncil/secrets.env", toolStateKeyLine())
	writeFile(t, root, ".devmap/repo_map.json", "{}\n")
	o := attackRun(t, root, "TASK-TS-IGNORED", benignChange(t, root))
	reportLeaks(t, o.result, o.gaps, key)
	for _, g := range o.gaps {
		if g.GapType == "security_risk" {
			t.Fatalf("an ignored tool-state file was read: %s", g.Description)
		}
	}
	if o.result.Status != "verified" || len(o.result.DegradedReasons) != 0 {
		t.Fatalf("an ignored tool-state file changed the verdict: status=%s degraded=%q", o.result.Status, o.result.DegradedReasons)
	}
}

// Nothing about tool state reaches any verdict but the secret scan: the same
// change with and without a tool-state file produces the same changed set, the
// same gap types, the same coverage and rigor account, and the diff the gates
// judge holds none of it.
func TestToolStateDoesNotChangeAnyOtherGate(t *testing.T) {
	build := func(withToolState bool) (attackOutcome, []string) {
		root := gitRepo(t)
		writeFile(t, root, ".devmap/seed.json", "{}\n")
		attackCommit(t, root, "track tool state")
		if withToolState {
			writeFile(t, root, ".devmap/seed.json", "{\"n\": 1}\n")
			writeFile(t, root, ".devcouncil/new.json", "{}\n")
		}
		o := attackRun(t, root, "TASK-TS-NEUTRAL", benignChange(t, root))
		return o, advTypes(o.gaps)
	}
	without, withoutTypes := build(false)
	with, withTypes := build(true)
	if !reflect.DeepEqual(withTypes, withoutTypes) {
		t.Fatalf("gap types moved: with %v, without %v", withTypes, withoutTypes)
	}
	if strings.Contains(with.diff, ".devmap") || strings.Contains(with.diff, ".devcouncil") {
		t.Fatalf("tool state reached the diff the gates judge:\n%s", with.diff)
	}
	for name, pair := range map[string][2]any{
		"status":             {with.result.Status, without.result.Status},
		"coverage measured":  {with.result.CoverageMeasured, without.result.CoverageMeasured},
		"coverage skipped":   {with.result.CoverageSkippedReason, without.result.CoverageSkippedReason},
		"rigor applied":      {with.result.RigorApplied, without.result.RigorApplied},
		"rigor skipped":      {with.result.RigorSkippedReason, without.result.RigorSkippedReason},
		"degraded reasons":   {with.result.DegradedReasons, without.result.DegradedReasons},
		"blocking gap count": {len(with.result.BlockingGaps), len(without.result.BlockingGaps)},
	} {
		if !reflect.DeepEqual(pair[0], pair[1]) {
			t.Errorf("%s moved: with %v, without %v", name, pair[0], pair[1])
		}
	}
}

// A change of tool state alone is not work the task did: it plants no orphan.
func TestToolStateIsNotAnOrphanOrAPlannedFileGap(t *testing.T) {
	root := gitRepo(t)
	writeFile(t, root, ".devmap/notes.txt", "notes\n")
	o := attackRun(t, root, "TASK-TS-ORPHAN", benignChange(t, root))
	for _, g := range o.gaps {
		if g.GapType == "orphan_diff" || g.GapType == "scope_violation" || (g.File != nil && strings.HasPrefix(*g.File, ".devmap")) {
			t.Fatalf("tool state became a scope gap: %s %s", g.GapType, g.Description)
		}
	}
}

// The key never appears in any output, wherever it is written under tool state.
func TestToolStateKeyNeverAppearsInAnyOutput(t *testing.T) {
	key := exampleAWSKey()
	for name, write := range map[string]func(t *testing.T, root string){
		"text": func(t *testing.T, root string) { writeFile(t, root, ".devmap/k.txt", toolStateKeyLine()) },
		"binary": func(t *testing.T, root string) {
			writeFile(t, root, ".devmap/k.bin", "\x00"+toolStateKeyLine())
		},
		"credential path": func(t *testing.T, root string) { writeFile(t, root, ".devcouncil/id_rsa", toolStateKeyLine()) },
	} {
		t.Run(name, func(t *testing.T) {
			root := gitRepo(t)
			write(t, root)
			o := attackRun(t, root, "TASK-TS-LEAK", benignChange(t, root))
			if bytes.Contains([]byte(everything(t, o)), []byte(key)) {
				t.Fatalf("the credential is in the run's output")
			}
		})
	}
}

// The collection keeps tool state out of the change and in its own diff: the
// changed set, the diff the gates judge and the subject hash are what they were,
// and the second diff holds the tracked and the untracked file.
func TestToolStateIsCollectedApartFromTheChange(t *testing.T) {
	root := gitRepo(t)
	writeFile(t, root, ".devmap/seed.json", "{}\n")
	attackCommit(t, root, "track tool state")
	benignChange(t, root)
	before, err := verify.CollectGitStateReport(context.Background(), root)
	if err != nil {
		t.Fatal(err)
	}
	subjectBefore, err := verify.WorkingTreeSubject(context.Background(), root)
	if err != nil {
		t.Fatal(err)
	}
	if before.ToolStateDiff != "" || len(before.ToolStateUnscanned) != 0 {
		t.Fatalf("tool state that did not change was collected: %q %q", before.ToolStateDiff, before.ToolStateUnscanned)
	}
	writeFile(t, root, ".devmap/seed.json", "{\"n\": 1}\n")
	writeFile(t, root, ".devcouncil/new.json", "{\"new\": true}\n")
	after, err := verify.CollectGitStateReport(context.Background(), root)
	if err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(after.Changed, before.Changed) || after.Diff != before.Diff || after.Empty != before.Empty {
		t.Fatalf("tool state moved the change: changed %q -> %q", before.Changed, after.Changed)
	}
	for _, want := range []string{"+++ b/.devmap/seed.json", `+{"n": 1}`, "+++ b/.devcouncil/new.json", `+{"new": true}`} {
		if !strings.Contains(after.ToolStateDiff, want) {
			t.Fatalf("the tool-state diff lacks %q:\n%s", want, after.ToolStateDiff)
		}
	}
	if strings.Contains(after.ToolStateDiff, "README") {
		t.Fatalf("the tool-state diff holds the change:\n%s", after.ToolStateDiff)
	}
	subjectAfter, err := verify.WorkingTreeSubject(context.Background(), root)
	if err != nil {
		t.Fatal(err)
	}
	if subjectAfter != subjectBefore {
		t.Fatalf("tool state moved the subject hash: %s -> %s", subjectBefore, subjectAfter)
	}
	changed, diff, empty, err := verify.CollectGitState(context.Background(), root)
	if err != nil || !reflect.DeepEqual(changed, after.Changed) || diff != after.Diff || empty != after.Empty {
		t.Fatalf("CollectGitState no longer returns the report's change: %v", err)
	}
}

// A repository with no commit yet has no HEAD to diff tool state against: what
// is staged under it is read from disk, with what is untracked.
func TestToolStateInAHeadlessRepositoryIsCollected(t *testing.T) {
	key := exampleAWSKey()
	root := t.TempDir()
	runGitIn(t, root, "init", "-q")
	writeFile(t, root, ".devcouncil/staged.txt", toolStateKeyLine())
	runGitIn(t, root, "add", ".devcouncil/staged.txt")
	writeFile(t, root, ".devmap/loose.txt", "loose\n")
	state, err := verify.CollectGitStateReport(context.Background(), root)
	if err != nil {
		t.Fatal(err)
	}
	for _, want := range []string{"+++ b/.devcouncil/staged.txt", "+++ b/.devmap/loose.txt"} {
		if !strings.Contains(state.ToolStateDiff, want) {
			t.Fatalf("the tool-state diff lacks %q:\n%s", want, state.ToolStateDiff)
		}
	}
	requireLooked(t, key, attackRun(t, root, "TASK-TS-HEADLESS"))
}

// More paths than are listed one by one are counted, not dropped: the reasons
// stay bounded and say there are more.
func TestToolStateReasonsAreBoundedAndSayThereAreMore(t *testing.T) {
	root := gitRepo(t)
	for i := 0; i < 120; i++ {
		writeFile(t, root, filepath.ToSlash(filepath.Join(".devmap", "bin", strings.Repeat("a", 1)+string(rune('a'+i%26))+string(rune('a'+i/26))+".bin")), "\x00blob\n")
	}
	state, err := verify.CollectGitStateReport(context.Background(), root)
	if err != nil {
		t.Fatal(err)
	}
	if len(state.ToolStateUnscanned) != 101 {
		t.Fatalf("%d reasons, want 100 listed and one that counts the rest", len(state.ToolStateUnscanned))
	}
	if last := state.ToolStateUnscanned[100]; last != "tool state not secret-scanned: 20 further path(s) not listed" {
		t.Fatalf("the last reason is %q", last)
	}
}
