// Skill discovery, delivery, and refusal contracts.
//
// Ported from the retired `tests/unit/test_devmap_skill_delivery.py`, which
// drove the Python `devcouncil.skills.registry` deleted in 3286db5. Every
// assertion below existed there; the ones this file does not carry are named
// in that commit's message as deliberately retired, not lost.
//
// The through-line is that a refusal must happen *before* any destination is
// touched. Three hosts share one corpus, so a run that writes `.claude` and
// then refuses `.agents` leaves the agent hosts disagreeing about what the
// guidance says — a worse state than not installing at all.
package skills_test

import (
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"testing"
	"testing/fstest"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/skills"
)

const contentionEnv = "DEVCOUNCIL_SKILLS_CONTENTION_ROOT"

// TestMain doubles as the worker for the cross-process contention test: an
// in-process goroutine cannot show that the lock survives a second `devcouncil`
// invocation, which is how the receipt is actually raced in the field.
func TestMain(m *testing.M) {
	if root := os.Getenv(contentionEnv); root != "" {
		if _, err := skills.Scaffold(skills.Options{
			Root:         root,
			Skills:       []skills.Skill{example("portable", "Complete")},
			Destinations: []string{".claude/skills", ".cursor/skills", ".agents/skills"},
		}); err != nil {
			os.Stderr.WriteString(err.Error() + "\n")
			os.Exit(1)
		}
		os.Exit(0)
	}
	os.Exit(m.Run())
}

func example(name, body string) skills.Skill {
	return skills.Skill{
		Name:    name,
		Content: []byte("---\nname: " + name + "\ndescription: Example workflow\n---\n" + body + "\n"),
	}
}

// entries names everything directly under root, so "nothing was written" is an
// assertion about the tree rather than about the one path a test looked at.
func entries(t *testing.T, root string) []string {
	t.Helper()
	found, err := os.ReadDir(root)
	if err != nil {
		t.Fatal(err)
	}
	names := make([]string, 0, len(found))
	for _, e := range found {
		names = append(names, e.Name())
	}
	return names
}

func mustScaffold(t *testing.T, root string, chosen ...skills.Skill) *skills.Result {
	t.Helper()
	result, err := skills.Scaffold(skills.Options{Root: root, Skills: chosen})
	if err != nil {
		t.Fatal(err)
	}
	return result
}

func TestUnsafeNamesFailBeforeAnyWrite(t *testing.T) {
	for _, name := range []string{
		"../escape", "/absolute", "a/b", `a\b`, "..", "", "CON", "con", "lpt1",
		strings.Repeat("a", 65),
	} {
		t.Run("name="+name, func(t *testing.T) {
			root := t.TempDir()
			// A valid skill goes first so a rejection cannot be mistaken for
			// "there was nothing to install".
			_, err := skills.Scaffold(skills.Options{
				Root:   root,
				Skills: []skills.Skill{example("example", "Original"), example(name, "Original")},
			})
			if err == nil {
				t.Fatalf("accepted unsafe skill name %q", name)
			}
			if got := entries(t, root); len(got) != 0 {
				t.Fatalf("wrote %v before refusing %q", got, name)
			}
		})
	}
}

func TestDestinationsStayInsideRequestedRepository(t *testing.T) {
	for _, dest := range []string{"../escape", "/absolute", `C:\escape`, "a/../../escape", `a\..\escape`} {
		t.Run("dest="+dest, func(t *testing.T) {
			root := t.TempDir()
			_, err := skills.Scaffold(skills.Options{
				Root:         root,
				Skills:       []skills.Skill{example("example", "Original")},
				Destinations: []string{dest},
			})
			if err == nil {
				t.Fatalf("accepted destination %q", dest)
			}
			if got := entries(t, root); len(got) != 0 {
				t.Fatalf("wrote %v before refusing %q", got, dest)
			}
		})
	}
}

func TestConflictingDuplicatesFailBeforeAnyWrite(t *testing.T) {
	root := t.TempDir()
	_, err := skills.Scaffold(skills.Options{
		Root:   root,
		Skills: []skills.Skill{example("example", "Original"), example("example", "Different")},
	})
	if err == nil || !strings.Contains(err.Error(), "duplicate") {
		t.Fatalf("want a duplicate refusal, got %v", err)
	}
	if got := entries(t, root); len(got) != 0 {
		t.Fatalf("wrote %v before refusing the duplicate", got)
	}
}

// A user edit in one host directory must not be overwritten, and must not cost
// the other hosts a half-applied upgrade.
func TestManagedUpgradePreservesUserEditsAndPreflightsEveryDestination(t *testing.T) {
	root := t.TempDir()
	mustScaffold(t, root, example("example", "Original"))
	mustScaffold(t, root, example("example", "Upgraded"))

	// `.cursor` sorts last of the three hosts, so an installer that checks and
	// writes in one pass reaches the other two before it ever sees the edit.
	edited := filepath.Join(root, ".cursor", "skills", "example", "SKILL.md")
	if err := os.WriteFile(edited, []byte("User-owned edits"), 0o644); err != nil {
		t.Fatal(err)
	}
	before := map[string][]byte{}
	for _, host := range []string{".agents", ".claude"} {
		path := filepath.Join(root, host, "skills", "example", "SKILL.md")
		body, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		before[path] = body
	}

	_, err := skills.Scaffold(skills.Options{
		Root: root, Skills: []skills.Skill{example("example", "Next")},
	})
	if err == nil {
		t.Fatal("overwrote a locally modified skill")
	}
	if !strings.Contains(err.Error(), "modified") && !strings.Contains(err.Error(), "unmanaged") {
		t.Fatalf("refusal does not say why: %v", err)
	}
	if got, _ := os.ReadFile(edited); string(got) != "User-owned edits" {
		t.Fatalf("user edit was overwritten: %q", got)
	}
	for path, want := range before {
		got, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		if string(got) != string(want) {
			t.Fatalf("a refused install still upgraded %s:\n before %q\n after  %q", path, want, got)
		}
	}
}

func TestConcurrentRepeatedInstallationsConvergeWithoutPartialFiles(t *testing.T) {
	root := t.TempDir()
	chosen := make([]skills.Skill, 20)
	for i := range chosen {
		chosen[i] = example("example-"+string(rune('a'+i)), "Original")
	}

	var mu sync.Mutex
	written := 0
	var wg sync.WaitGroup
	for i := 0; i < 36; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			result, err := skills.Scaffold(skills.Options{Root: root, Skills: chosen})
			if err != nil {
				t.Error(err)
				return
			}
			mu.Lock()
			written += len(result.Files)
			mu.Unlock()
		}()
	}
	wg.Wait()

	// 20 skills x 3 hosts, each written exactly once across all 36 callers.
	if written != 60 {
		t.Fatalf("want 60 writes across every caller, got %d", written)
	}
	if result := mustScaffold(t, root, chosen...); len(result.Files) != 0 {
		t.Fatalf("a converged tree still reported work: %v", result.Files)
	}
	installed, temporaries := 0, []string{}
	err := filepath.WalkDir(root, func(path string, d os.DirEntry, err error) error {
		if err != nil || d.IsDir() {
			return err
		}
		switch {
		case d.Name() == "SKILL.md":
			installed++
		case strings.HasSuffix(d.Name(), ".tmp"):
			temporaries = append(temporaries, path)
		}
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	if installed != 60 {
		t.Fatalf("want 60 SKILL.md, got %d", installed)
	}
	if len(temporaries) != 0 {
		t.Fatalf("left partial files behind: %v", temporaries)
	}
}

func TestSymlinkTargetDoesNotRedirectInstallation(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("symlink creation needs elevation on Windows")
	}
	base := t.TempDir()
	outside := filepath.Join(base, "outside")
	root := filepath.Join(base, "repo")
	for _, dir := range []string{outside, root} {
		if err := os.Mkdir(dir, 0o755); err != nil {
			t.Fatal(err)
		}
	}
	if err := os.Symlink(outside, filepath.Join(root, ".agents")); err != nil {
		t.Fatal(err)
	}
	_, err := skills.Scaffold(skills.Options{
		Root: root, Skills: []skills.Skill{example("example", "Original")},
	})
	if err == nil || !strings.Contains(err.Error(), "symlink") {
		t.Fatalf("want a symlink refusal, got %v", err)
	}
	if got := entries(t, outside); len(got) != 0 {
		t.Fatalf("installation followed the symlink: %v", got)
	}
	if _, err := os.Stat(filepath.Join(root, ".claude")); !os.IsNotExist(err) {
		t.Fatal("a refused install still wrote another host")
	}
}

// The size is read from the directory entry: opening an 8 MiB skill to discover
// it is 8 MiB is the failure mode this bound exists to prevent.
func TestOversizedExistingSkillIsRefusedWithoutReadingAllOfIt(t *testing.T) {
	root := t.TempDir()
	target := filepath.Join(root, ".agents", "skills", "example", "SKILL.md")
	if err := os.MkdirAll(filepath.Dir(target), 0o755); err != nil {
		t.Fatal(err)
	}
	file, err := os.Create(target)
	if err != nil {
		t.Fatal(err)
	}
	if err := file.Truncate(8 * 1024 * 1024); err != nil {
		t.Fatal(err)
	}
	if err := file.Close(); err != nil {
		t.Fatal(err)
	}
	_, err = skills.Scaffold(skills.Options{
		Root: root, Skills: []skills.Skill{example("example", "Original")},
	})
	if err == nil || !strings.Contains(err.Error(), "limit") {
		t.Fatalf("want a byte-limit refusal, got %v", err)
	}
	if _, err := os.Stat(filepath.Join(root, ".claude")); !os.IsNotExist(err) {
		t.Fatal("a refused install still wrote another host")
	}
}

// A receipt that cannot be parsed is not a receipt saying "we own these files".
func TestCorruptReceiptIsNotPermissionToOverwrite(t *testing.T) {
	for _, raw := range []string{
		"{broken",
		"[]",
		`{"schema":2,"files":{}}`,
		`{"schema":true,"files":{}}`,
		`{"schema":1,"files":{"x":1}}`,
	} {
		t.Run(raw, func(t *testing.T) {
			root := t.TempDir()
			receipt := filepath.Join(root, ".devcouncil-skills.json")
			if err := os.WriteFile(receipt, []byte(raw), 0o644); err != nil {
				t.Fatal(err)
			}
			_, err := skills.Scaffold(skills.Options{
				Root: root, Skills: []skills.Skill{example("example", "Original")},
			})
			if err == nil || !strings.Contains(err.Error(), "receipt") {
				t.Fatalf("want a receipt refusal, got %v", err)
			}
			if got := entries(t, root); len(got) != 1 || got[0] != ".devcouncil-skills.json" {
				t.Fatalf("wrote %v past a corrupt receipt", got)
			}
		})
	}
}

// A write that fails partway must leave a tree the next run can finish, not a
// receipt claiming files that are not there and not a lock nobody will release.
func TestPartialIOFailureIsVisibleRecoverableAndReleasesLock(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("directory mode bits do not deny writes on Windows")
	}
	if os.Geteuid() == 0 {
		t.Skip("root ignores the mode bits this injection depends on")
	}
	root := t.TempDir()
	blocked := filepath.Join(root, ".cursor", "skills")
	if err := os.MkdirAll(blocked, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.Chmod(blocked, 0o500); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.Chmod(blocked, 0o755) })

	_, err := skills.Scaffold(skills.Options{
		Root: root, Skills: []skills.Skill{example("example", "Original")},
	})
	if err == nil {
		t.Fatal("an unwritable destination reported success")
	}
	if _, err := os.Stat(filepath.Join(root, ".devcouncil-skills.lock")); !os.IsNotExist(err) {
		t.Fatal("the lock outlived the failure")
	}
	if _, err := os.Stat(filepath.Join(root, ".devcouncil-skills.json")); !os.IsNotExist(err) {
		t.Fatal("a failed install still wrote a receipt")
	}

	if err := os.Chmod(blocked, 0o755); err != nil {
		t.Fatal(err)
	}
	recovered := mustScaffold(t, root, example("example", "Original"))
	if len(recovered.Files) != 1 {
		t.Fatalf("recovery should finish only the unwritten host, wrote %v", recovered.Files)
	}
	if again := mustScaffold(t, root, example("example", "Original")); len(again.Files) != 0 {
		t.Fatalf("recovery did not converge: %v", again.Files)
	}
}

// The wait is bounded *and* real: a lock that is refused instantly would pass a
// "returns an error" assertion while stealing every concurrent install.
func TestBusyLockHasBoundedWaitAndIsNotStolen(t *testing.T) {
	root := t.TempDir()
	lock := filepath.Join(root, ".devcouncil-skills.lock")
	if err := os.Mkdir(lock, 0o755); err != nil {
		t.Fatal(err)
	}
	start := time.Now()
	_, err := skills.Scaffold(skills.Options{
		Root: root, Skills: []skills.Skill{example("example", "Original")},
	})
	elapsed := time.Since(start)
	if err == nil || !strings.Contains(err.Error(), "busy") {
		t.Fatalf("want a busy refusal, got %v", err)
	}
	if elapsed < skills.LockWait {
		t.Fatalf("gave up after %s without waiting out the %s lock", elapsed, skills.LockWait)
	}
	if elapsed > 4*skills.LockWait {
		t.Fatalf("waited %s on a %s bound", elapsed, skills.LockWait)
	}
	if got := entries(t, root); len(got) != 1 || got[0] != ".devcouncil-skills.lock" {
		t.Fatalf("stole the lock or wrote past it: %v", got)
	}
}

// Installing skills must not leave a state directory behind: `.devcouncil/` is
// how this repository's own tooling decides a project has opted in.
func TestInstallDoesNotCreateAStateMarker(t *testing.T) {
	root := t.TempDir()
	mustScaffold(t, root, example("example", "Original"))
	if _, err := os.Stat(filepath.Join(root, ".devcouncil")); !os.IsNotExist(err) {
		t.Fatal("install created a .devcouncil marker")
	}
}

func TestProcessContentionKeepsOneCompleteReceipt(t *testing.T) {
	root := t.TempDir()
	var wg sync.WaitGroup
	failures := make(chan string, 16)
	for i := 0; i < 16; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			cmd := exec.Command(os.Args[0])
			cmd.Env = append(os.Environ(), contentionEnv+"="+root)
			if out, err := cmd.CombinedOutput(); err != nil {
				failures <- err.Error() + ": " + string(out)
			}
		}()
	}
	wg.Wait()
	close(failures)
	for message := range failures {
		t.Fatalf("a concurrent installer failed: %s", message)
	}

	installed := 0
	if err := filepath.WalkDir(root, func(_ string, d os.DirEntry, err error) error {
		if err == nil && !d.IsDir() && d.Name() == "SKILL.md" {
			installed++
		}
		return err
	}); err != nil {
		t.Fatal(err)
	}
	if installed != 3 {
		t.Fatalf("want one skill per host, got %d", installed)
	}
	raw, err := os.ReadFile(filepath.Join(root, ".devcouncil-skills.json"))
	if err != nil {
		t.Fatal(err)
	}
	var receipt struct {
		Files map[string]string `json:"files"`
	}
	if err := json.Unmarshal(raw, &receipt); err != nil {
		t.Fatalf("contention left an unparseable receipt %q: %v", raw, err)
	}
	if len(receipt.Files) != 3 {
		t.Fatalf("contention truncated the receipt to %d entries: %v", len(receipt.Files), receipt.Files)
	}
}

// The mode is checked before the open, because opening a FIFO blocks until a
// writer arrives — a refusal that hangs is not a refusal.
func TestNamedPipeIsRefusedWithoutBlocking(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("POSIX named pipe probe")
	}
	root := t.TempDir()
	target := filepath.Join(root, ".agents", "skills", "example", "SKILL.md")
	if err := os.MkdirAll(filepath.Dir(target), 0o755); err != nil {
		t.Fatal(err)
	}
	if out, err := exec.Command("mkfifo", target).CombinedOutput(); err != nil {
		t.Skipf("mkfifo unavailable: %v: %s", err, out)
	}
	done := make(chan error, 1)
	go func() {
		_, err := skills.Scaffold(skills.Options{
			Root: root, Skills: []skills.Skill{example("example", "Original")},
		})
		done <- err
	}()
	select {
	case err := <-done:
		if err == nil || !strings.Contains(err.Error(), "regular file") {
			t.Fatalf("want a regular-file refusal, got %v", err)
		}
	case <-time.After(10 * time.Second):
		t.Fatal("the installer blocked on a named pipe")
	}
}

func TestAllThreeHostsReceiveIdenticalBytesUnderUnicodeSpacePaths(t *testing.T) {
	root := filepath.Join(t.TempDir(), "répo with spaces [fixture]")
	if err := os.Mkdir(root, 0o755); err != nil {
		t.Fatal(err)
	}
	all, err := skills.Embedded.Load()
	if err != nil {
		t.Fatal(err)
	}
	if len(all) == 0 {
		t.Fatal("the embedded library is empty")
	}
	result := mustScaffold(t, root, all...)
	if want := len(all) * len(skills.DefaultDestinations); len(result.Files) != want {
		t.Fatalf("want %d files, got %d", want, len(result.Files))
	}
	for _, skill := range all {
		var seen [][]byte
		for _, host := range skills.DefaultDestinations {
			body, err := os.ReadFile(filepath.Join(root, filepath.FromSlash(host), skill.Name, "SKILL.md"))
			if err != nil {
				t.Fatal(err)
			}
			seen = append(seen, body)
		}
		for _, body := range seen[1:] {
			if string(body) != string(seen[0]) {
				t.Fatalf("%s differs between hosts", skill.Name)
			}
		}
	}
}

// A check must be able to say "missing" and "up to date" differently, or its
// success means only that it ran.
func TestCheckReportsWhatIsMissingAndNeverWrites(t *testing.T) {
	root := t.TempDir()
	chosen := example("example", "Original")

	missing, err := skills.Scaffold(skills.Options{
		Root: root, Skills: []skills.Skill{chosen}, CheckOnly: true,
	})
	if err != nil {
		t.Fatal(err)
	}
	if len(missing.Files) != len(skills.DefaultDestinations) {
		t.Fatalf("check did not name the missing files: %v", missing.Files)
	}
	if got := entries(t, root); len(got) != 0 {
		t.Fatalf("--check wrote %v", got)
	}

	mustScaffold(t, root, chosen)
	stamps := map[string]time.Time{}
	if err := filepath.WalkDir(root, func(path string, d os.DirEntry, err error) error {
		if err != nil || d.IsDir() {
			return err
		}
		info, err := d.Info()
		if err != nil {
			return err
		}
		stamps[path] = info.ModTime()
		return nil
	}); err != nil {
		t.Fatal(err)
	}

	clean, err := skills.Scaffold(skills.Options{
		Root: root, Skills: []skills.Skill{chosen}, CheckOnly: true,
	})
	if err != nil {
		t.Fatal(err)
	}
	if len(clean.Files) != 0 {
		t.Fatalf("check reported work on an installed tree: %v", clean.Files)
	}
	for path, stamp := range stamps {
		info, err := os.Stat(path)
		if err != nil {
			t.Fatal(err)
		}
		if !info.ModTime().Equal(stamp) {
			t.Fatalf("--check rewrote %s", path)
		}
	}
}

// "No file here" and "an empty file here" are different answers. Conflating
// them leaves a zero-byte skill uninstalled on every run while each run reports
// the tree as converged.
func TestAnEmptySkillIsInstalledRatherThanReportedAsAlreadyThere(t *testing.T) {
	root := t.TempDir()
	blank := skills.Skill{Name: "blank", Content: []byte{}}

	first, err := skills.Scaffold(skills.Options{Root: root, Skills: []skills.Skill{blank}})
	if err != nil {
		t.Fatal(err)
	}
	if len(first.Files) != len(skills.DefaultDestinations) {
		t.Fatalf("an empty skill was skipped: %v", first.Files)
	}
	for _, host := range skills.DefaultDestinations {
		path := filepath.Join(root, filepath.FromSlash(host), "blank", "SKILL.md")
		info, err := os.Stat(path)
		if err != nil {
			t.Fatalf("%s was never created: %v", path, err)
		}
		if info.Size() != 0 {
			t.Fatalf("%s is %d bytes, want 0", path, info.Size())
		}
	}
	again, err := skills.Scaffold(skills.Options{Root: root, Skills: []skills.Skill{blank}})
	if err != nil {
		t.Fatal(err)
	}
	if len(again.Files) != 0 {
		t.Fatalf("an installed empty skill still reported work: %v", again.Files)
	}
}

// A skill whose frontmatter cannot be read must be reported. Falling back to
// the filename installs it under a name no agent host was told to look for.
func TestLoadReportsAnInvalidSkillInsteadOfSilentlyRenamingIt(t *testing.T) {
	for _, bad := range []struct{ label, body string }{
		{"no frontmatter", "# Body only\n"},
		{"unterminated", "---\nname: broken\ndescription: Example\n"},
		{"no name", "---\ndescription: Example\n---\nBody\n"},
		{"empty name", "---\nname:\ndescription: Example\n---\nBody\n"},
	} {
		t.Run(bad.label, func(t *testing.T) {
			library := skills.Library{FS: fstest.MapFS{
				"broken.md": &fstest.MapFile{Data: []byte(bad.body)},
			}}
			if _, err := library.Load(); err == nil || !strings.Contains(err.Error(), "broken.md") {
				t.Fatalf("want an error naming broken.md, got %v", err)
			}
		})
	}
}
