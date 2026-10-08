package skills_test

import (
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/skills"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// Scaffold installs through `devmap skills install --library-stdin`, so these
// drive the real binary. The installer's own contracts — all-or-nothing
// batches, the receipt, the lock, symlink and size refusals — are tested
// beside it in `rust/devmap-cli/src/skills.rs`; what is here is that this
// library reaches it intact and that its answers come back.
func useDevmap(t *testing.T) {
	t.Helper()
	t.Setenv("DEVMAP_BIN", testsupport.Devmap(t))
}

// devmapLockWait is `LOCK_TIMEOUT` in rust/devmap-cli/src/skills.rs.
const devmapLockWait = 5 * time.Second

func coreEngineering(t *testing.T) skills.Skill {
	t.Helper()
	all, err := skills.Embedded.Load()
	if err != nil {
		t.Fatal(err)
	}
	for _, s := range all {
		if s.Name == "core-engineering" {
			return s
		}
	}
	t.Fatal("core-engineering missing from the embedded library")
	return skills.Skill{}
}

func TestEmbeddedLibraryHasEighteenSkills(t *testing.T) {
	all, err := skills.Embedded.Load()
	if err != nil {
		t.Fatal(err)
	}
	if len(all) != 18 {
		t.Fatalf("want 18 domain skills, got %d", len(all))
	}
}

// The language policy is installed under the name its frontmatter declares, so
// a renamed or mistyped header would ship guidance no agent host asks for.
func TestEmbeddedLibraryCarriesLanguagePolicy(t *testing.T) {
	all, err := skills.Embedded.Load()
	if err != nil {
		t.Fatal(err)
	}
	for _, s := range all {
		if s.Name == "language-policy" {
			return
		}
	}
	t.Fatal("language-policy missing from the embedded library")
}

func TestScaffoldCoreEngineering(t *testing.T) {
	useDevmap(t)
	root := t.TempDir()
	core := coreEngineering(t)
	res, err := skills.Scaffold(skills.Options{
		Root: root, Skills: []skills.Skill{core},
		Destinations: []string{".claude/skills", ".cursor/skills"},
	})
	if err != nil {
		t.Fatal(err)
	}
	if len(res.Files) != 2 {
		t.Fatalf("files: %v", res.Files)
	}
	for _, rel := range res.Files {
		if _, err := os.Stat(filepath.Join(root, rel)); err != nil {
			t.Fatal(err)
		}
	}
	for _, rel := range res.Files {
		got, err := os.ReadFile(filepath.Join(root, rel))
		if err != nil {
			t.Fatal(err)
		}
		if string(got) != string(core.Content) {
			t.Fatalf("%s is not the embedded skill byte for byte", rel)
		}
	}
	if _, err := os.Stat(filepath.Join(root, ".devcouncil-skills.json")); err != nil {
		t.Fatal(err)
	}
	again, err := skills.Scaffold(skills.Options{
		Root: root, Skills: []skills.Skill{core}, CheckOnly: true,
		Destinations: []string{".claude/skills", ".cursor/skills"},
	})
	if err != nil {
		t.Fatal(err)
	}
	if len(again.Files) != 0 {
		t.Fatalf("a check of an installed tree reported work: %v", again.Files)
	}
}

// A check that finds work is an answer, not an error: the missing files come
// back so the caller can exit non-zero naming them.
func TestScaffoldCheckNamesWhatIsMissing(t *testing.T) {
	useDevmap(t)
	root := t.TempDir()
	res, err := skills.Scaffold(skills.Options{
		Root: root, Skills: []skills.Skill{coreEngineering(t)}, CheckOnly: true,
		Destinations: []string{".cursor/skills"},
	})
	if err != nil {
		t.Fatal(err)
	}
	if len(res.Files) != 1 || res.Files[0] != ".cursor/skills/core-engineering/SKILL.md" {
		t.Fatalf("check should name the missing file, got %v", res.Files)
	}
	if entries, _ := os.ReadDir(root); len(entries) != 0 {
		t.Fatalf("a check wrote: %v", entries)
	}
}

// Without a devmap there is no installer, and that is a refusal.
func TestScaffoldWithoutDevmapRefuses(t *testing.T) {
	t.Setenv("DEVMAP_BIN", "")
	t.Setenv("PATH", t.TempDir())
	root := t.TempDir()
	if _, err := skills.Scaffold(skills.Options{Root: root, Skills: []skills.Skill{coreEngineering(t)}}); err == nil {
		t.Fatal("installed without an installer")
	}
	if entries, _ := os.ReadDir(root); len(entries) != 0 {
		t.Fatalf("wrote without devmap: %v", entries)
	}
}

func TestScaffoldRefusesUnowned(t *testing.T) {
	useDevmap(t)
	root := t.TempDir()
	core := coreEngineering(t)
	target := filepath.Join(root, ".cursor", "skills", "core-engineering", "SKILL.md")
	if err := os.MkdirAll(filepath.Dir(target), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(target, []byte("locally modified"), 0o644); err != nil {
		t.Fatal(err)
	}
	_, err := skills.Scaffold(skills.Options{
		Root: root, Skills: []skills.Skill{core},
		Destinations: []string{".cursor/skills"},
	})
	if err == nil {
		t.Fatal("expected unowned refusal")
	}
}

// A held lock must not block a read-only plan: --dry-run and --check answer
// questions about the tree and write nothing, so waiting on another installer
// would make them useless exactly while one is running.
//
// The lock's own timeout contract is
// `a_busy_lock_is_waited_out_within_its_bound_and_not_stolen` in
// rust/devmap-cli/src/skills.rs; this test deliberately takes the path that
// skips it.
func TestDryRunDoesNotWaitOnAHeldLock(t *testing.T) {
	useDevmap(t)
	root := t.TempDir()
	if err := os.Mkdir(filepath.Join(root, ".devcouncil-skills.lock"), 0o755); err != nil {
		t.Fatal(err)
	}
	core := coreEngineering(t)
	start := time.Now()
	result, err := skills.Scaffold(skills.Options{
		Root: root, Skills: []skills.Skill{core}, DryRun: true,
		Destinations: []string{".cursor/skills"},
	})
	if err != nil {
		t.Fatal(err)
	}
	if elapsed := time.Since(start); elapsed >= devmapLockWait {
		t.Fatalf("a dry run waited %s on a lock it never needed", elapsed)
	}
	if len(result.Files) != 1 {
		t.Fatalf("dry run should name the one missing file, got %v", result.Files)
	}
	if entries, err := os.ReadDir(root); err != nil || len(entries) != 1 {
		t.Fatalf("dry run changed the tree: %v (%v)", entries, err)
	}
}
