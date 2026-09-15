package skills_test

import (
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/skills"
)

func TestEmbeddedLibraryHasSeventeenSkills(t *testing.T) {
	all, err := skills.Embedded.Load()
	if err != nil {
		t.Fatal(err)
	}
	if len(all) != 17 {
		t.Fatalf("want 17 domain skills, got %d", len(all))
	}
}

func TestScaffoldCoreEngineering(t *testing.T) {
	root := t.TempDir()
	all, err := skills.Embedded.Load()
	if err != nil {
		t.Fatal(err)
	}
	var core *skills.Skill
	for i := range all {
		if all[i].Name == "core-engineering" {
			core = &all[i]
			break
		}
	}
	if core == nil {
		t.Fatal("core-engineering missing")
	}
	res, err := skills.Scaffold(skills.Options{
		Root: root, Skills: []skills.Skill{*core},
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
	if _, err := os.Stat(filepath.Join(root, ".devcouncil-skills.json")); err != nil {
		t.Fatal(err)
	}
}

func TestScaffoldRefusesUnowned(t *testing.T) {
	root := t.TempDir()
	all, _ := skills.Embedded.Load()
	var core skills.Skill
	for _, s := range all {
		if s.Name == "core-engineering" {
			core = s
			break
		}
	}
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
// The lock's own timeout contract is TestBusyLockHasBoundedWaitAndIsNotStolen
// in delivery_test.go; this test deliberately takes the path that skips it.
func TestDryRunDoesNotWaitOnAHeldLock(t *testing.T) {
	root := t.TempDir()
	if err := os.Mkdir(filepath.Join(root, ".devcouncil-skills.lock"), 0o755); err != nil {
		t.Fatal(err)
	}
	all, err := skills.Embedded.Load()
	if err != nil {
		t.Fatal(err)
	}
	var core skills.Skill
	for _, s := range all {
		if s.Name == "core-engineering" {
			core = s
			break
		}
	}
	start := time.Now()
	result, err := skills.Scaffold(skills.Options{
		Root: root, Skills: []skills.Skill{core}, DryRun: true,
		Destinations: []string{".cursor/skills"},
	})
	if err != nil {
		t.Fatal(err)
	}
	if elapsed := time.Since(start); elapsed >= skills.LockWait {
		t.Fatalf("a dry run waited %s on a lock it never needed", elapsed)
	}
	if len(result.Files) != 1 {
		t.Fatalf("dry run should name the one missing file, got %v", result.Files)
	}
	if entries, err := os.ReadDir(root); err != nil || len(entries) != 1 {
		t.Fatalf("dry run changed the tree: %v (%v)", entries, err)
	}
}
