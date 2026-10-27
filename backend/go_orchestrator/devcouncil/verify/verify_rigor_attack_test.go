package verify_test

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/dcverify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// Attack tests for the verify-rigor lane (card ft-b9ef3ccf4f898d019658aa5ca4bbd11c).
// Contract: every byte a change introduces is scanned, or the run is refused or
// degraded. It never passes. A failing test here is a finding.

func attackSecretLine() string { return "let aws_key = \"" + exampleAWSKey() + "\";\n" }

// attackOutcome runs the host's collect-then-verify path. refused is the
// collection error (a refusal is a fail-closed outcome).
type attackOutcome struct {
	refused error
	empty   bool
	diff    string
	gaps    []verify.Gap
	result  verify.MCPResult
}

func attackRun(t *testing.T, root, id string, planned ...dc.PlannedFile) attackOutcome {
	t.Helper()
	changed, diff, empty, err := verify.CollectGitState(context.Background(), root)
	if err != nil {
		return attackOutcome{refused: err}
	}
	in := verify.Input{
		Task:     &store.Task{ID: id, Difficulty: "easy", PlannedFiles: planned},
		GateMode: "enforce", ChangedFiles: changed, DiffContent: diff, DiffEmpty: empty,
		WorkPresent: !empty, Rigor: dcverify.New(testsupport.DCVerify(t), root),
	}
	gaps, meta := verify.Run(context.Background(), in)
	return attackOutcome{empty: empty, diff: diff, gaps: gaps, result: verify.ToMCP(id, gaps, meta)}
}

// requireLooked fails unless the change was refused, or the run found the
// credential, or the run reports it could not look. A run that says nothing
// about the credential is the failure.
func requireLooked(t *testing.T, key string, o attackOutcome) {
	t.Helper()
	if o.refused != nil {
		if strings.Contains(o.refused.Error(), key) {
			t.Fatal("the refusal quotes the credential")
		}
		return
	}
	reportLeaks(t, o.result, o.gaps, key)
	for _, g := range o.gaps {
		if g.GapType == "security_risk" || g.GapType == "rigor_check_unavailable" {
			if o.result.Passed || o.result.Status == "verified" {
				t.Fatalf("status=%s passed=%v with a %s gap", o.result.Status, o.result.Passed, g.GapType)
			}
			return
		}
	}
	t.Fatalf("a credential in the change was never looked at: empty=%v status=%s passed=%v types=%v degraded=%v",
		o.empty, o.result.Status, o.result.Passed, advTypes(o.gaps), o.result.DegradedReasons)
}

// benignChange edits a tracked file innocently and returns its plan entry, so
// a run that misses the real change still has real work to verify and cannot
// be blocked by "nothing changed".
func benignChange(t *testing.T, root string) dc.PlannedFile {
	t.Helper()
	writeFile(t, root, "README", "base\nan innocent edit\n")
	return dc.PlannedFile{Path: "README", AllowedChange: dc.ChangeModify}
}

func attackCommit(t *testing.T, root, msg string) {
	t.Helper()
	runGitIn(t, root, "add", "-A")
	runGitIn(t, root, "commit", "-qm", msg)
}

// ---------------------------------------------------------------- tracked/untracked

// A file removed from the index and recreated is "deleted" to `git diff HEAD`
// (the diff carries a deletion section) and untracked to ls-files. The host
// treats the name as present in the diff and never reads the new bytes.
func TestAttackDeletedThenRecreatedUntrackedFileIsScanned(t *testing.T) {
	key := exampleAWSKey()
	root := gitRepo(t)
	writeFile(t, root, "src/f.rs", "pub fn f() {}\n")
	attackCommit(t, root, "add f")
	runGitIn(t, root, "rm", "-q", "--cached", "src/f.rs")
	writeFile(t, root, "src/f.rs", attackSecretLine())
	o := attackRun(t, root, "TASK-ATK-RECREATE",
		dc.PlannedFile{Path: "src/f.rs", AllowedChange: dc.ChangeModify})
	if o.refused == nil && strings.Contains(o.diff, key) {
		return
	}
	requireLooked(t, key, o)
}

func TestAttackStagedDeleteThenRecreatedFileIsScanned(t *testing.T) {
	key := exampleAWSKey()
	root := gitRepo(t)
	writeFile(t, root, "src/g.rs", "pub fn g() {}\n")
	attackCommit(t, root, "add g")
	runGitIn(t, root, "rm", "-q", "src/g.rs")
	writeFile(t, root, "src/g.rs", attackSecretLine())
	o := attackRun(t, root, "TASK-ATK-RM-RECREATE",
		dc.PlannedFile{Path: "src/g.rs", AllowedChange: dc.ChangeModify})
	requireLooked(t, key, o)
}

// `a\b.txt` (a tracked file whose name holds a backslash) and `a/b.txt` (an
// untracked file) are different paths. The host folds `\` to `/` for both and
// then dedupes, so the untracked file is judged already present.
func TestAttackBackslashTrackedNameHidesAnUntrackedSecret(t *testing.T) {
	key := exampleAWSKey()
	root := gitRepo(t)
	writeFile(t, root, `a\b.txt`, "one\n")
	attackCommit(t, root, "backslash name")
	writeFile(t, root, `a\b.txt`, "two\n")
	writeFile(t, root, "a/b.txt", attackSecretLine())
	o := attackRun(t, root, "TASK-ATK-BSLASH",
		dc.PlannedFile{Path: "a/b.txt", AllowedChange: dc.ChangeCreate},
		dc.PlannedFile{Path: `a\b.txt`, AllowedChange: dc.ChangeModify})
	t.Logf("refused=%v diffHasKey=%v types=%v", o.refused, strings.Contains(o.diff, key), advTypes(o.gaps))
	if o.refused == nil && strings.Contains(o.diff, key) {
		return
	}
	requireLooked(t, key, o)
}

// The same collision through TrimSpace: the tracked `f.txt ` and the untracked
// `f.txt` are one key.
func TestAttackTrailingSpaceTrackedNameHidesAnUntrackedSecret(t *testing.T) {
	key := exampleAWSKey()
	root := gitRepo(t)
	writeFile(t, root, "f.txt ", "one\n")
	attackCommit(t, root, "trailing space name")
	writeFile(t, root, "f.txt ", "two\n")
	writeFile(t, root, "f.txt", attackSecretLine())
	o := attackRun(t, root, "TASK-ATK-TRAILSP",
		dc.PlannedFile{Path: "f.txt", AllowedChange: dc.ChangeCreate})
	if o.refused == nil && strings.Contains(o.diff, key) {
		return
	}
	requireLooked(t, key, o)
}

// Legitimate untracked names must be collected, not refused.
func TestAttackLegitUntrackedNamesAreCollected(t *testing.T) {
	names := []string{
		"-dash.txt",
		"tab\tname.txt",
		"trailing.txt ",
		" leading.txt",
		"é.txt", // NFD
		"é.txt",  // NFC
		"dir with space/file name.txt",
		"quote'single.txt",
		"dollar$name.txt",
	}
	for i, name := range names {
		t.Run(fmt.Sprintf("%d", i), func(t *testing.T) {
			root := gitRepo(t)
			writeFile(t, root, name, "INNOCENT-CONTENT\n")
			_, diff, _, err := verify.CollectGitState(context.Background(), root)
			if err != nil {
				t.Fatalf("a legitimate untracked name %q was refused: %v", name, err)
			}
			if !strings.Contains(diff, "INNOCENT-CONTENT") {
				t.Fatalf("the bytes of %q are not in the diff", name)
			}
		})
	}
}

// A hard-linked file is refused, and the refusal says why. A hard link can
// share an inode with a file outside the repository, so reading it is a read
// outside the root; refusing is the security fail-closed choice. The operator
// has to be able to tell that from a read that failed, so the message names
// the reason. (Round 1 of the attack expected the bytes to be collected; the
// owner decided the refusal stays and its message is what is pinned.)
func TestAttackHardLinkedUntrackedFileIsCollected(t *testing.T) {
	root := gitRepo(t)
	writeFile(t, root, "one.txt", "INNOCENT-CONTENT\n")
	if err := os.Link(filepath.Join(root, "one.txt"), filepath.Join(root, "two.txt")); err != nil {
		t.Skipf("hard links unsupported: %v", err)
	}
	_, _, _, err := verify.CollectGitState(context.Background(), root)
	if err == nil {
		t.Fatal("a hard-linked untracked file was collected without a refusal")
	}
	if !strings.Contains(err.Error(), "refused: hard-linked file") {
		t.Fatalf("the refusal does not say why: %v", err)
	}
	if strings.Contains(err.Error(), "INNOCENT-CONTENT") {
		t.Fatal("the refusal quotes the file's bytes")
	}
}

// Deletion, rename, mode-only change, empty new file: all legitimate and none
// may block the rigor gates as "unavailable".
func TestAttackLegitChangeKindsVerify(t *testing.T) {
	cases := map[string]func(t *testing.T, root string) []dc.PlannedFile{
		"deletion": func(t *testing.T, root string) []dc.PlannedFile {
			writeFile(t, root, "gone.rs", "pub fn gone() {}\n")
			attackCommit(t, root, "gone")
			runGitIn(t, root, "rm", "-q", "gone.rs")
			return []dc.PlannedFile{{Path: "gone.rs", AllowedChange: dc.ChangeDelete}}
		},
		"rename": func(t *testing.T, root string) []dc.PlannedFile {
			writeFile(t, root, "old.rs", "pub fn moved() -> u32 { 1 }\n")
			attackCommit(t, root, "old")
			runGitIn(t, root, "mv", "old.rs", "renamed.rs")
			return []dc.PlannedFile{{Path: "renamed.rs", AllowedChange: dc.ChangeModify}}
		},
		"mode only": func(t *testing.T, root string) []dc.PlannedFile {
			writeFile(t, root, "run.sh", "#!/bin/sh\necho hi\n")
			attackCommit(t, root, "run")
			if err := os.Chmod(filepath.Join(root, "run.sh"), 0o755); err != nil {
				t.Fatal(err)
			}
			return []dc.PlannedFile{{Path: "run.sh", AllowedChange: dc.ChangeModify}}
		},
		"empty untracked file": func(t *testing.T, root string) []dc.PlannedFile {
			writeFile(t, root, "pkg/__init__.py", "")
			return []dc.PlannedFile{{Path: "pkg/__init__.py", AllowedChange: dc.ChangeCreate}}
		},
		"space and unicode name": func(t *testing.T, root string) []dc.PlannedFile {
			writeFile(t, root, "my docs/résumé notes.md", "# notes\n")
			return []dc.PlannedFile{{Path: "my docs/résumé notes.md", AllowedChange: dc.ChangeCreate}}
		},
	}
	for name, setup := range cases {
		t.Run(name, func(t *testing.T) {
			root := gitRepo(t)
			planned := setup(t, root)
			o := attackRun(t, root, "TASK-ATK-LEGIT", planned...)
			if o.refused != nil {
				t.Fatalf("a legitimate %s was refused: %v", name, o.refused)
			}
			for _, g := range o.gaps {
				if g.GapType == "rigor_check_unavailable" {
					t.Fatalf("a legitimate %s blocked the rigor gates: %s", name, g.Description)
				}
			}
			if o.result.RigorSkippedReason != "" && !o.empty {
				t.Fatalf("a legitimate %s skipped rigor: %q", name, o.result.RigorSkippedReason)
			}
		})
	}
}

// ---------------------------------------------------------------- submodules

func submoduleRepoWith(t *testing.T, files map[string]string) (root, subDir string) {
	t.Helper()
	root = gitRepo(t)
	src := filepath.Join(t.TempDir(), "sub-src")
	if err := os.MkdirAll(src, 0o755); err != nil {
		t.Fatal(err)
	}
	runGitIn(t, src, "init", "-q")
	for name, body := range files {
		writeFile(t, src, name, body)
	}
	runGitIn(t, src, "add", "-A")
	runGitIn(t, src, "commit", "-qm", "s")
	runGitIn(t, root, "-c", "protocol.file.allow=always", "submodule", "add", "-q", src, "sub")
	runGitIn(t, root, "commit", "-qm", "sub")
	return root, filepath.Join(root, "sub")
}

func subPlanned(rel string) []dc.PlannedFile {
	return []dc.PlannedFile{
		{Path: "sub", AllowedChange: dc.ChangeModify},
		{Path: rel, AllowedChange: dc.ChangeModify},
	}
}

// git itself calls a submodule's untracked file "modified content". Nothing in
// the superproject's tracked or untracked listing names it.
func TestAttackUntrackedFileInsideASubmoduleIsScanned(t *testing.T) {
	key := exampleAWSKey()
	root, subDir := submoduleRepoWith(t, map[string]string{"s.txt": "ok\n"})
	writeFile(t, subDir, "new_secret.txt", attackSecretLine())
	o := attackRun(t, root, "TASK-ATK-SUB-UNTRACKED", append(subPlanned("sub/new_secret.txt"), benignChange(t, root))...)
	requireLooked(t, key, o)
	if o.refused == nil && o.empty {
		t.Fatal("the submodule's untracked file was collected as an empty tree")
	}
}

func TestAttackStagedOnlySecretInASubmoduleIsScanned(t *testing.T) {
	key := exampleAWSKey()
	root, subDir := submoduleRepoWith(t, map[string]string{"s.txt": "ok\n"})
	writeFile(t, subDir, "staged.txt", attackSecretLine())
	runGitIn(t, subDir, "add", "staged.txt")
	o := attackRun(t, root, "TASK-ATK-SUB-STAGED", subPlanned("sub/staged.txt")...)
	requireLooked(t, key, o)
}

// A gitlink in the index with no .gitmodules at all: git still describes the
// checked-out repository as a submodule with modified content.
func TestAttackGitlinkWithoutGitmodulesIsScanned(t *testing.T) {
	key := exampleAWSKey()
	root, subDir := submoduleRepoWith(t, map[string]string{"s.txt": "ok\n"})
	runGitIn(t, root, "rm", "-q", ".gitmodules")
	runGitIn(t, root, "commit", "-qm", "drop .gitmodules, keep the gitlink")
	writeFile(t, subDir, "s.txt", attackSecretLine())
	o := attackRun(t, root, "TASK-ATK-NOGITMODULES", append(subPlanned("sub/s.txt"), benignChange(t, root))...)
	requireLooked(t, key, o)
	if o.refused == nil && o.empty {
		t.Fatal("a modified checked-out gitlink was collected as an empty tree")
	}
}

// A modified tracked file whose name ends in a space: the walk trims the name,
// finds no such file, and records a deletion.
func TestAttackSubmoduleFileWithTrailingSpaceNameIsScanned(t *testing.T) {
	key := exampleAWSKey()
	root, subDir := submoduleRepoWith(t, map[string]string{"s.txt": "ok\n", "t.txt ": "one\n"})
	writeFile(t, subDir, "t.txt ", attackSecretLine())
	o := attackRun(t, root, "TASK-ATK-SUB-SPACE", subPlanned("sub/t.txt ")...)
	if o.refused == nil && strings.Contains(o.diff, key) {
		return
	}
	requireLooked(t, key, o)
}

func TestAttackSubmoduleFileWithSpaceAndUnicodeNameIsScanned(t *testing.T) {
	key := exampleAWSKey()
	name := "résumé notes.txt"
	root, subDir := submoduleRepoWith(t, map[string]string{"s.txt": "ok\n", name: "one\n"})
	writeFile(t, subDir, name, attackSecretLine())
	o := attackRun(t, root, "TASK-ATK-SUB-UNI", subPlanned("sub/"+name)...)
	requireLooked(t, key, o)
	if o.refused != nil {
		t.Fatalf("a legitimate modified submodule file was refused: %v", o.refused)
	}
}

// ---------------------------------------------------------------- nesting

func nestedRepo(t *testing.T, levels int) (root, leafDir string) {
	t.Helper()
	t.Setenv("GIT_CONFIG_COUNT", "1")
	t.Setenv("GIT_CONFIG_KEY_0", "protocol.file.allow")
	t.Setenv("GIT_CONFIG_VALUE_0", "always")
	base := t.TempDir()
	leaf := filepath.Join(base, fmt.Sprintf("r%d", levels))
	if err := os.MkdirAll(leaf, 0o755); err != nil {
		t.Fatal(err)
	}
	runGitIn(t, leaf, "init", "-q")
	writeFile(t, leaf, "leaf.txt", "ok\n")
	runGitIn(t, leaf, "add", "-A")
	runGitIn(t, leaf, "commit", "-qm", "leaf")
	prev := leaf
	for i := levels - 1; i >= 1; i-- {
		dir := filepath.Join(base, fmt.Sprintf("r%d", i))
		if err := os.MkdirAll(dir, 0o755); err != nil {
			t.Fatal(err)
		}
		runGitIn(t, dir, "init", "-q")
		writeFile(t, dir, "f.txt", "ok\n")
		runGitIn(t, dir, "add", "-A")
		runGitIn(t, dir, "commit", "-qm", "base")
		runGitIn(t, dir, "submodule", "add", "-q", prev, "m")
		runGitIn(t, dir, "commit", "-qm", "m")
		prev = dir
	}
	root = gitRepo(t)
	runGitIn(t, root, "submodule", "add", "-q", prev, "m")
	runGitIn(t, root, "commit", "-qm", "m")
	runGitIn(t, root, "submodule", "update", "--init", "--recursive", "-q")
	leafDir = root
	for i := 0; i < levels; i++ {
		leafDir = filepath.Join(leafDir, "m")
	}
	return root, leafDir
}

func nestedRel(levels int, file string) string {
	parts := make([]string, 0, levels+1)
	for i := 0; i < levels; i++ {
		parts = append(parts, "m")
	}
	return strings.Join(append(parts, file), "/")
}

// Ten levels is past the cap of 8. A secret at the bottom must be refused or
// reported as unavailable, never passed.
func TestAttackSubmoduleNestedNineDeepIsRefusedNotPassed(t *testing.T) {
	key := exampleAWSKey()
	root, leafDir := nestedRepo(t, 9)
	writeFile(t, leafDir, "leaf.txt", attackSecretLine())
	o := attackRun(t, root, "TASK-ATK-NEST9",
		dc.PlannedFile{Path: "m", AllowedChange: dc.ChangeModify},
		dc.PlannedFile{Path: nestedRel(9, "leaf.txt"), AllowedChange: dc.ChangeModify})
	requireLooked(t, key, o)
}

// Eight levels is inside the cap: a legitimate nested change is read.
func TestAttackSubmoduleNestedEightDeepIsScanned(t *testing.T) {
	key := exampleAWSKey()
	root, leafDir := nestedRepo(t, 8)
	writeFile(t, leafDir, "leaf.txt", attackSecretLine())
	o := attackRun(t, root, "TASK-ATK-NEST8",
		dc.PlannedFile{Path: "m", AllowedChange: dc.ChangeModify},
		dc.PlannedFile{Path: nestedRel(8, "leaf.txt"), AllowedChange: dc.ChangeModify})
	if o.refused != nil {
		t.Fatalf("a change 8 submodules deep, inside the cap, was refused: %v", o.refused)
	}
	requireLooked(t, key, o)
}

// An untracked secret in a nested submodule's worktree.
func TestAttackUntrackedSecretInNestedSubmoduleIsScanned(t *testing.T) {
	key := exampleAWSKey()
	root, leafDir := nestedRepo(t, 2)
	writeFile(t, leafDir, "new.txt", attackSecretLine())
	o := attackRun(t, root, "TASK-ATK-NEST-UNTRACKED",
		dc.PlannedFile{Path: "m", AllowedChange: dc.ChangeModify},
		dc.PlannedFile{Path: nestedRel(2, "new.txt"), AllowedChange: dc.ChangeCreate},
		benignChange(t, root))
	requireLooked(t, key, o)
}

// ---------------------------------------------------------------- git that hangs

// A git that answers `version` and then never returns must not hold the
// verification past the caller's deadline.
func TestAttackHangingGitHonoursTheCallersDeadline(t *testing.T) {
	realGit := testsupport.Tool(t, "git")
	bin := t.TempDir()
	script := "#!/bin/sh\nif [ \"$1\" = \"version\" ] || [ \"$1\" = \"--version\" ]; then exec " + realGit + " \"$@\"; fi\nexec sleep 25\n"
	if err := os.WriteFile(filepath.Join(bin, "git"), []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	root := gitRepo(t)
	writeFile(t, root, "new.txt", "INNOCENT\n")
	t.Setenv("PATH", bin+string(os.PathListSeparator)+os.Getenv("PATH"))
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	start := time.Now()
	done := make(chan error, 1)
	go func() {
		_, _, _, err := verify.CollectGitState(ctx, root)
		done <- err
	}()
	select {
	case err := <-done:
		if err == nil {
			t.Fatal("a git that never answered was read as a clean tree")
		}
		if took := time.Since(start); took > 15*time.Second {
			t.Fatalf("the 3s deadline was honoured after %s", took)
		}
	case <-time.After(20 * time.Second):
		t.Fatal("CollectGitState ignored the caller's 3s deadline for 20s")
	}
}

// ---------------------------------------------------------------- stubs behind binary markers

func requireStubLooked(t *testing.T, o attackOutcome) {
	t.Helper()
	if o.refused != nil {
		return
	}
	for _, g := range o.gaps {
		if g.GapType == "stub_detected" || g.GapType == "rigor_check_unavailable" {
			return
		}
	}
	if len(o.result.DegradedReasons) > 0 && !o.result.Passed {
		return
	}
	t.Fatalf("an added todo!() was never looked at: status=%s passed=%v types=%v degraded=%v diff=%q",
		o.result.Status, o.result.Passed, advTypes(o.gaps), o.result.DegradedReasons, o.diff)
}

// One NUL byte makes the untracked file "binary" to the host's synthetic diff.
// The secret scan then reads it back from disk; the stub scan has no hunk and
// never sees `todo!()`.
func TestAttackNulPrefixedSourceFileIsStubScanned(t *testing.T) {
	root := gitRepo(t)
	writeBytes(t, root, "src/new.rs", []byte("\x00\npub fn work() -> u32 {\n    todo!()\n}\n"))
	o := attackRun(t, root, "TASK-ATK-NUL-STUB",
		dc.PlannedFile{Path: "src/new.rs", AllowedChange: dc.ChangeCreate})
	requireStubLooked(t, o)
}

// `*.rs -diff` makes git print `Binary files differ` for an edited source file.
func TestAttackDiffAttributeSourceFileIsStubScanned(t *testing.T) {
	root := gitRepo(t)
	writeFile(t, root, ".gitattributes", "*.rs -diff\n")
	writeFile(t, root, "src/work.rs", "pub fn work() -> u32 {\n    1\n}\n")
	attackCommit(t, root, "attrs")
	writeFile(t, root, "src/work.rs", "pub fn work() -> u32 {\n    todo!()\n}\n")
	o := attackRun(t, root, "TASK-ATK-DIFFATTR-STUB",
		dc.PlannedFile{Path: "src/work.rs", AllowedChange: dc.ChangeModify})
	requireStubLooked(t, o)
}

func TestAttackDiffAttributeTextFileSecretIsScanned(t *testing.T) {
	key := exampleAWSKey()
	root := gitRepo(t)
	writeFile(t, root, ".gitattributes", "*.txt -diff\n")
	writeFile(t, root, "notes.txt", "one\n")
	attackCommit(t, root, "attrs")
	writeFile(t, root, "notes.txt", attackSecretLine())
	o := attackRun(t, root, "TASK-ATK-DIFFATTR-SECRET",
		dc.PlannedFile{Path: "notes.txt", AllowedChange: dc.ChangeModify})
	requireLooked(t, key, o)
}

// A task with no planned files has no in-scope set for the binary read to
// walk, and no planned file for an orphan check to name.
func TestAttackNoPlannedFilesBinarySecretIsNotPassed(t *testing.T) {
	key := exampleAWSKey()
	root := gitRepo(t)
	writeBytes(t, root, "blob.bin", append([]byte{0}, []byte(attackSecretLine())...))
	o := attackRun(t, root, "TASK-ATK-NOPLAN")
	if o.refused == nil && (o.result.Passed || o.result.Status == "verified") {
		t.Fatalf("a binary file holding a credential verified with no plan: types=%v", advTypes(o.gaps))
	}
	reportLeaks(t, o.result, o.gaps, key)
}

// Tracked files with names git C-quotes in the diff header: the credential is
// found and attributed.
func TestAttackTrackedHostileNamesWithSecretAreScanned(t *testing.T) {
	key := exampleAWSKey()
	names := []string{"odd\nname.txt", "tab\tname.txt", `back\slash.txt`, "-leading.txt", "q\"uote.txt", "éé.txt", "sp ace/b/in b/x.txt"}
	for i, name := range names {
		t.Run(fmt.Sprintf("%d", i), func(t *testing.T) {
			root := gitRepo(t)
			writeFile(t, root, name, "one\n")
			attackCommit(t, root, "add")
			writeFile(t, root, name, attackSecretLine())
			o := attackRun(t, root, "TASK-ATK-HOSTILE", dc.PlannedFile{Path: name, AllowedChange: dc.ChangeModify})
			requireLooked(t, key, o)
		})
	}
}

func TestAttackRenameThatGainsASecretIsScanned(t *testing.T) {
	key := exampleAWSKey()
	root := gitRepo(t)
	writeFile(t, root, "new b/keep.txt", "keep\n")
	writeFile(t, root, "old b/name.txt", "line one\nline two\nline three\nline four\nline five\n")
	attackCommit(t, root, "add")
	runGitIn(t, root, "mv", "old b/name.txt", "new b/name.txt")
	writeFile(t, root, "new b/name.txt", "line one\nline two\nline three\nline four\nline five\n"+attackSecretLine())
	o := attackRun(t, root, "TASK-ATK-RENAME", dc.PlannedFile{Path: "new b/name.txt", AllowedChange: dc.ChangeModify})
	requireLooked(t, key, o)
}

// ---------------------------------------------------------------- tool state

// .devmap and .devcouncil are excluded from the changed set and from the diff
// as "tool-owned state". A credential written there is a byte the change
// introduced, committed with the rest.
func TestAttackSecretUnderToolStateDirIsNotInvisible(t *testing.T) {
	t.Skip("owner decision pending on ft-926e5b45: whether tool-state directories are secret-scanned")
	key := exampleAWSKey()
	root := gitRepo(t)
	writeFile(t, root, ".devmap/notes/keys.txt", attackSecretLine())
	o := attackRun(t, root, "TASK-ATK-TOOLSTATE",
		dc.PlannedFile{Path: ".devmap/notes/keys.txt", AllowedChange: dc.ChangeCreate},
		benignChange(t, root))
	requireLooked(t, key, o)
}
