package proc

import (
	"errors"
	"fmt"
	"math/rand/v2"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

func writeExecutable(t *testing.T, path string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte("#!/bin/sh\nexit 0\n"), 0o755); err != nil {
		t.Fatal(err)
	}
}

// A build output inside the repository is repository content. Finding it on
// PATH does not make it a trusted installation.
func TestLookPathOutsideRefusesRepositoryLocalCandidate(t *testing.T) {
	base := t.TempDir()
	root := filepath.Join(base, "repo")
	writeExecutable(t, filepath.Join(root, "target", "release", "tool-under-test"))
	t.Setenv("PATH", filepath.Join(root, "target", "release"))

	got, err := LookPathOutside("tool-under-test", root)
	if !errors.Is(err, ErrRepositoryLocal) {
		t.Fatalf("resolved a repository-local candidate: %q (err %v)", got, err)
	}
}

// The spelling of the candidate is not the question; its identity is. A PATH
// entry that is a symlink into the repository is still repository content.
func TestLookPathOutsideRefusesSymlinkIntoRepository(t *testing.T) {
	base := t.TempDir()
	root := filepath.Join(base, "repo")
	bin := filepath.Join(base, "bin")
	writeExecutable(t, filepath.Join(root, "target", "release", "tool-under-test"))
	if err := os.MkdirAll(bin, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(filepath.Join(root, "target", "release", "tool-under-test"),
		filepath.Join(bin, "tool-under-test")); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", bin)

	got, err := LookPathOutside("tool-under-test", root)
	if !errors.Is(err, ErrRepositoryLocal) {
		t.Fatalf("followed a symlink into the repository: %q (err %v)", got, err)
	}
}

// An alternate spelling of the same root is the same root.
func TestLookPathOutsideRefusesUncanonicalRootSpelling(t *testing.T) {
	base := t.TempDir()
	root := filepath.Join(base, "repo")
	writeExecutable(t, filepath.Join(root, "bin", "tool-under-test"))
	t.Setenv("PATH", filepath.Join(root, "bin"))

	spelled := filepath.Join(root, "sub", "..")
	if err := os.MkdirAll(filepath.Join(root, "sub"), 0o755); err != nil {
		t.Fatal(err)
	}
	got, err := LookPathOutside("tool-under-test", spelled)
	if !errors.Is(err, ErrRepositoryLocal) {
		t.Fatalf("an alternate root spelling let repository content through: %q (err %v)", got, err)
	}
}

// A genuine installation outside the repository is still selected.
func TestLookPathOutsideAcceptsInstallationOutsideRepository(t *testing.T) {
	base := t.TempDir()
	root := filepath.Join(base, "repo")
	if err := os.MkdirAll(root, 0o755); err != nil {
		t.Fatal(err)
	}
	installed := filepath.Join(base, "usr", "bin", "tool-under-test")
	writeExecutable(t, installed)
	t.Setenv("PATH", filepath.Dir(installed))

	got, err := LookPathOutside("tool-under-test", root)
	if err != nil {
		t.Fatalf("refused a legitimate installation: %v", err)
	}
	wantReal, err := filepath.EvalSymlinks(installed)
	if err != nil {
		t.Fatal(err)
	}
	if got != wantReal {
		t.Fatalf("resolved %q, want %q", got, wantReal)
	}
}

// A directory named like the program is not a program.
func TestLookPathOutsideRefusesNonRegularCandidate(t *testing.T) {
	base := t.TempDir()
	root := filepath.Join(base, "repo")
	if err := os.MkdirAll(root, 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", filepath.Join(base, "empty"))
	if _, err := LookPathOutside("tool-under-test", root); err == nil {
		t.Fatal("resolved a program that is not on PATH")
	}
}

// componentFixture returns a repository root and an empty home, with PATH
// holding nothing.
func componentFixture(t *testing.T) (root, home string) {
	t.Helper()
	base := t.TempDir()
	root = filepath.Join(base, "repo")
	home = filepath.Join(base, "home")
	for _, dir := range []string{root, home, filepath.Join(base, "empty")} {
		if err := os.MkdirAll(dir, 0o755); err != nil {
			t.Fatal(err)
		}
	}
	t.Setenv("PATH", filepath.Join(base, "empty"))
	t.Setenv("HOME", home)
	return root, home
}

// ~/.local/bin is searched when PATH has no candidate at all.
func TestResolveComponentFallsBackToLocalBin(t *testing.T) {
	root, home := componentFixture(t)
	installed := filepath.Join(home, ".local", "bin", "tool-under-test")
	writeExecutable(t, installed)

	got, err := ResolveComponent("tool-under-test", root, Override{})
	if err != nil {
		t.Fatalf("did not search ~/.local/bin: %v", err)
	}
	want, err := filepath.EvalSymlinks(installed)
	if err != nil {
		t.Fatal(err)
	}
	if got != want {
		t.Fatalf("resolved %q, want %q", got, want)
	}
}

// A refused PATH candidate stays refused. Falling through to ~/.local/bin
// would hide that something in the repository is on PATH.
func TestResolveComponentDoesNotFallPastARefusal(t *testing.T) {
	root, home := componentFixture(t)
	writeExecutable(t, filepath.Join(home, ".local", "bin", "tool-under-test"))
	writeExecutable(t, filepath.Join(root, "bin", "tool-under-test"))
	t.Setenv("PATH", filepath.Join(root, "bin"))

	got, err := ResolveComponent("tool-under-test", root, Override{})
	if !errors.Is(err, ErrRepositoryLocal) {
		t.Fatalf("a refusal fell through to another rung: %q (err %v)", got, err)
	}
}

// The fallback rung is held to the same check: a home directory that is the
// repository does not make its ~/.local/bin a trusted installation.
func TestResolveComponentRefusesLocalBinInsideRoot(t *testing.T) {
	_, home := componentFixture(t)
	writeExecutable(t, filepath.Join(home, ".local", "bin", "tool-under-test"))

	got, err := ResolveComponent("tool-under-test", home, Override{})
	if !errors.Is(err, ErrRepositoryLocal) {
		t.Fatalf("resolved a repository-local ~/.local/bin candidate: %q (err %v)", got, err)
	}
}

// A file that is not executable is not a program, on either rung.
func TestResolveComponentRefusesNonExecutableLocalBin(t *testing.T) {
	root, home := componentFixture(t)
	path := filepath.Join(home, ".local", "bin", "tool-under-test")
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte("not a program"), 0o644); err != nil {
		t.Fatal(err)
	}

	if got, err := ResolveComponent("tool-under-test", root, Override{}); err == nil {
		t.Fatalf("resolved a file without the execute bit: %q", got)
	}
}

// Found nowhere is distinguishable from refused, so a caller can say
// "install it" rather than "something in the repository is on PATH".
func TestResolveComponentNotFoundIsErrNotFound(t *testing.T) {
	root, _ := componentFixture(t)

	_, err := ResolveComponent("tool-under-test", root, Override{})
	if !errors.Is(err, exec.ErrNotFound) {
		t.Fatalf("err = %v, want exec.ErrNotFound", err)
	}
}

// An override is used as named, including a path inside the repository:
// setting it is an operator's act, not something the repository arranges.
func TestResolveComponentOverrideIsUsed(t *testing.T) {
	root, _ := componentFixture(t)
	inside := filepath.Join(root, "bin", "tool-under-test")
	writeExecutable(t, inside)

	got, err := ResolveComponent("tool-under-test", root, Override{Path: inside, Source: "TOOL_BIN"})
	if err != nil || got != inside {
		t.Fatalf("the override was not honoured: %q (err %v)", got, err)
	}
}

// An override that names nothing, or a directory, is refused and never
// replaced by a search, and the refusal names where it came from.
func TestResolveComponentOverrideIsRefusedNotReplaced(t *testing.T) {
	root, home := componentFixture(t)
	writeExecutable(t, filepath.Join(home, ".local", "bin", "tool-under-test"))
	for _, path := range []string{filepath.Join(root, "missing"), root} {
		got, err := ResolveComponent("tool-under-test", root, Override{Path: path, Source: "TOOL_BIN"})
		if err == nil {
			t.Fatalf("override %s was replaced by %q", path, got)
		}
		if !strings.Contains(err.Error(), "TOOL_BIN") {
			t.Fatalf("refusal does not name its source: %v", err)
		}
	}
}

// Whitespace is not a path.
func TestEnvOverrideTreatsBlankAsUnset(t *testing.T) {
	t.Setenv("TOOL_BIN", "  ")
	if o := EnvOverride("TOOL_BIN"); o.Path != "" || o.Source != "TOOL_BIN" {
		t.Fatalf("EnvOverride = %+v", o)
	}
}

// A relative PATH entry names a directory relative to wherever the process
// happens to run, which is the repository. It is refused, and the refusal
// does not fall through to ~/.local/bin.
func TestResolveComponentRefusesRelativePATHEntry(t *testing.T) {
	root, home := componentFixture(t)
	writeExecutable(t, filepath.Join(root, "bin", "tool-under-test"))
	writeExecutable(t, filepath.Join(home, ".local", "bin", "tool-under-test"))
	t.Setenv("PATH", "bin")
	t.Chdir(root)

	got, err := ResolveComponent("tool-under-test", root, Override{})
	if err == nil {
		t.Fatalf("resolved %q through a relative PATH entry", got)
	}
	if errors.Is(err, exec.ErrNotFound) {
		t.Fatalf("a relative-entry refusal read as not found: %v", err)
	}
}

// What was checked is what runs. A relative override is resolved against
// the working directory once, here, and handed on absolute: a client that
// runs in the repository root would otherwise execute a different file from
// the one that was checked.
func TestResolveComponentOverrideIsMadeAbsolute(t *testing.T) {
	root, _ := componentFixture(t)
	cwd := t.TempDir()
	checked := filepath.Join(cwd, "bin", "tool-under-test")
	writeExecutable(t, checked)
	writeExecutable(t, filepath.Join(root, "bin", "tool-under-test"))
	t.Chdir(cwd)

	got, err := ResolveComponent("tool-under-test", root, Override{Path: "bin/tool-under-test", Source: "TOOL_BIN"})
	if err != nil {
		t.Fatal(err)
	}
	if !filepath.IsAbs(got) {
		t.Fatalf("override handed on relative: %q", got)
	}
	gotReal, err := filepath.EvalSymlinks(got)
	if err != nil {
		t.Fatal(err)
	}
	wantReal, err := filepath.EvalSymlinks(checked)
	if err != nil {
		t.Fatal(err)
	}
	if gotReal != wantReal {
		t.Fatalf("resolved %q, want the file that was checked, %q", gotReal, wantReal)
	}
}

// sameDirOrSkip skips when alias does not name dir on this filesystem, so the
// spelling tests below run where the filesystem folds spellings (macOS APFS,
// Windows NTFS) and are skipped where it does not.
func sameDirOrSkip(t *testing.T, dir, alias string) {
	t.Helper()
	a, err := os.Stat(dir)
	if err != nil {
		t.Fatal(err)
	}
	b, err := os.Stat(alias)
	if err != nil || !os.SameFile(a, b) {
		t.Skipf("%s does not name %s on this filesystem", alias, dir)
	}
}

// Containment is a question about identity, not spelling. On a filesystem
// that ignores case, a PATH entry that spells the repository in another case
// names the same directory, and a program in it is still repository content.
func TestLookPathOutsideRefusesCaseVariantOfRoot(t *testing.T) {
	base := t.TempDir()
	root := filepath.Join(base, "Repo")
	writeExecutable(t, filepath.Join(root, "bin", "tool-under-test"))
	for _, alias := range []string{"REPO", "repo"} {
		sameDirOrSkip(t, root, filepath.Join(base, alias))
		t.Setenv("PATH", filepath.Join(base, alias, "bin"))
		got, err := LookPathOutside("tool-under-test", root)
		if !errors.Is(err, ErrRepositoryLocal) {
			t.Fatalf("PATH spelled %s let repository content through: %q (err %v)", alias, got, err)
		}
	}
}

// The same holds for Unicode normalization: APFS treats a composed and a
// decomposed name as one directory.
func TestLookPathOutsideRefusesNormalizationVariantOfRoot(t *testing.T) {
	base := t.TempDir()
	root := filepath.Join(base, "café")
	writeExecutable(t, filepath.Join(root, "bin", "tool-under-test"))
	alias := filepath.Join(base, "café")
	sameDirOrSkip(t, root, alias)
	t.Setenv("PATH", filepath.Join(alias, "bin"))

	got, err := LookPathOutside("tool-under-test", root)
	if !errors.Is(err, ErrRepositoryLocal) {
		t.Fatalf("a decomposed spelling let repository content through: %q (err %v)", got, err)
	}
}

// The root's spelling can differ too: --project-root typed in another case
// is the same repository.
func TestLookPathOutsideRefusesCaseVariantOfRootArgument(t *testing.T) {
	base := t.TempDir()
	root := filepath.Join(base, "Repo")
	writeExecutable(t, filepath.Join(root, "bin", "tool-under-test"))
	alias := filepath.Join(base, "rEPO")
	sameDirOrSkip(t, root, alias)
	t.Setenv("PATH", filepath.Join(root, "bin"))

	got, err := LookPathOutside("tool-under-test", alias)
	if !errors.Is(err, ErrRepositoryLocal) {
		t.Fatalf("a root spelled in another case let repository content through: %q (err %v)", got, err)
	}
}

// The ~/.local/bin rung is held to the same identity check.
func TestResolveComponentRefusesLocalBinUnderCaseVariantRoot(t *testing.T) {
	_, home := componentFixture(t)
	writeExecutable(t, filepath.Join(home, ".local", "bin", "tool-under-test"))
	alias := filepath.Join(filepath.Dir(home), strings.ToUpper(filepath.Base(home)))
	sameDirOrSkip(t, home, alias)

	got, err := ResolveComponent("tool-under-test", alias, Override{})
	if !errors.Is(err, ErrRepositoryLocal) {
		t.Fatalf("resolved %q under a root spelled in another case (err %v)", got, err)
	}
}

// A root that does not exist contains nothing; the program is found and the
// caller's own command reports the missing root.
func TestLookPathOutsideTreatsMissingRootAsContainingNothing(t *testing.T) {
	base := t.TempDir()
	installed := filepath.Join(base, "usr", "bin", "tool-under-test")
	writeExecutable(t, installed)
	t.Setenv("PATH", filepath.Dir(installed))

	if _, err := LookPathOutside("tool-under-test", filepath.Join(base, "no-such-repo")); err != nil {
		t.Fatalf("a missing root refused an installation outside it: %v", err)
	}
}

func symlinkOrSkip(t *testing.T, target, link string) {
	t.Helper()
	if runtime.GOOS == "windows" {
		t.Skip("symlink fixtures need unix semantics")
	}
	if err := os.MkdirAll(filepath.Dir(link), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(target, link); err != nil {
		t.Fatal(err)
	}
}

// A link the repository carries chooses the program even when the program
// lives outside: the chain outside -> repository link -> outside target is
// refused at the repository's link.
func TestLookPathOutsideRefusesAChainThroughTheRepository(t *testing.T) {
	base := t.TempDir()
	root := filepath.Join(base, "repo")
	installed := filepath.Join(base, "usr", "bin", "real-tool")
	writeExecutable(t, installed)
	symlinkOrSkip(t, installed, filepath.Join(root, "links", "tool-under-test"))
	symlinkOrSkip(t, filepath.Join(root, "links", "tool-under-test"), filepath.Join(base, "bin", "tool-under-test"))
	t.Setenv("PATH", filepath.Join(base, "bin"))

	got, err := LookPathOutside("tool-under-test", root)
	if !errors.Is(err, ErrRepositoryLocal) {
		t.Fatalf("a chain through a repository link was accepted: %q (err %v)", got, err)
	}
}

// A chain that never touches the repository is an ordinary installation —
// the shape of Homebrew's /opt/homebrew/bin links — and is used by its end.
func TestLookPathOutsideFollowsAChainOutsideTheRepository(t *testing.T) {
	base := t.TempDir()
	root := filepath.Join(base, "repo")
	if err := os.MkdirAll(root, 0o755); err != nil {
		t.Fatal(err)
	}
	installed := filepath.Join(base, "Cellar", "tool", "1.0", "bin", "tool-under-test")
	writeExecutable(t, installed)
	symlinkOrSkip(t, "../Cellar/tool/1.0/bin/tool-under-test", filepath.Join(base, "bin", "tool-under-test"))
	t.Setenv("PATH", filepath.Join(base, "bin"))

	got, err := LookPathOutside("tool-under-test", root)
	if err != nil {
		t.Fatalf("refused an installation reached by links outside the repository: %v", err)
	}
	want, err := filepath.EvalSymlinks(installed)
	if err != nil {
		t.Fatal(err)
	}
	if got != want {
		t.Fatalf("resolved %q, want %q", got, want)
	}
}

// A link loop ends in an error, not a hang.
func TestLookPathOutsideBoundsALinkLoop(t *testing.T) {
	base := t.TempDir()
	root := filepath.Join(base, "repo")
	if err := os.MkdirAll(root, 0o755); err != nil {
		t.Fatal(err)
	}
	a := filepath.Join(base, "bin", "tool-under-test")
	b := filepath.Join(base, "loop", "b")
	symlinkOrSkip(t, b, a)
	symlinkOrSkip(t, a, b)
	t.Setenv("PATH", filepath.Join(base, "bin"))
	if got, err := LookPathOutside("tool-under-test", root); err == nil {
		t.Fatalf("a link loop resolved to %q", got)
	}
}

// TestLookPathOutsideAgainstGeneratedLayouts builds many PATH layouts from a
// fixed seed and checks each answer against what the layout was built to be,
// not against the code under test: for every PATH entry the generator records
// whether the first executable it would yield was placed, or linked, through
// the repository. The first entry yielding one decides. Stated rather than
// assumed: exec.LookPath's first match is the candidate.
func TestLookPathOutsideAgainstGeneratedLayouts(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("symlink fixtures need unix semantics")
	}
	const layouts = 300
	rng := rand.New(rand.NewPCG(20261008, 1))
	for i := 0; i < layouts; i++ {
		base := t.TempDir()
		root := filepath.Join(base, "Repo")
		outside := filepath.Join(base, "outside")
		writeExecutable(t, filepath.Join(outside, "real", "tool-under-test"))
		writeExecutable(t, filepath.Join(root, "build", "tool-under-test"))
		folds := false
		if a, err := os.Stat(root); err == nil {
			if b, err := os.Stat(filepath.Join(base, "REPO")); err == nil && os.SameFile(a, b) {
				folds = true
			}
		}

		type entry struct {
			dir        string
			executable bool
			inside     bool
		}
		var entries []entry
		n := 1 + rng.IntN(4)
		for j := 0; j < n; j++ {
			dir := filepath.Join(base, fmt.Sprintf("e%d", j))
			if err := os.MkdirAll(dir, 0o755); err != nil {
				t.Fatal(err)
			}
			link := filepath.Join(dir, "tool-under-test")
			switch kind := rng.IntN(7); kind {
			case 0: // nothing here
				entries = append(entries, entry{dir: dir})
			case 1: // a real program outside
				writeExecutable(t, link)
				entries = append(entries, entry{dir: dir, executable: true})
			case 2: // a link to the outside program
				symlinkOrSkip(t, filepath.Join(outside, "real", "tool-under-test"), link)
				entries = append(entries, entry{dir: dir, executable: true})
			case 3: // a link to the repository's build output
				symlinkOrSkip(t, filepath.Join(root, "build", "tool-under-test"), link)
				entries = append(entries, entry{dir: dir, executable: true, inside: true})
			case 4: // a link to a repository link that points outside
				repoLink := filepath.Join(root, fmt.Sprintf("l%d", j), "tool-under-test")
				symlinkOrSkip(t, filepath.Join(outside, "real", "tool-under-test"), repoLink)
				symlinkOrSkip(t, repoLink, link)
				entries = append(entries, entry{dir: dir, executable: true, inside: true})
			case 5: // the repository's directory itself, spelled as built or folded
				spelled := root
				if folds && rng.IntN(2) == 0 {
					spelled = filepath.Join(base, "REPO")
				}
				entries = append(entries, entry{dir: filepath.Join(spelled, "build"), executable: true, inside: true})
			case 6: // a directory link into the repository
				if err := os.Remove(dir); err != nil {
					t.Fatal(err)
				}
				symlinkOrSkip(t, filepath.Join(root, "build"), dir)
				entries = append(entries, entry{dir: dir, executable: true, inside: true})
			}
		}
		var dirs []string
		for _, e := range entries {
			dirs = append(dirs, e.dir)
		}
		t.Setenv("PATH", strings.Join(dirs, string(os.PathListSeparator)))

		got, err := LookPathOutside("tool-under-test", root)
		var decider *entry
		for k := range entries {
			if entries[k].executable {
				decider = &entries[k]
				break
			}
		}
		switch {
		case decider == nil:
			if !errors.Is(err, exec.ErrNotFound) {
				t.Fatalf("layout %d (PATH=%v): nothing to find, got %q err %v", i, dirs, got, err)
			}
		case decider.inside:
			if !errors.Is(err, ErrRepositoryLocal) {
				t.Fatalf("layout %d (PATH=%v): repository content accepted: %q err %v", i, dirs, got, err)
			}
		default:
			if err != nil {
				t.Fatalf("layout %d (PATH=%v): installation outside refused: %v", i, dirs, err)
			}
			if within, _ := filepath.Rel(root, got); !strings.HasPrefix(within, "..") {
				t.Fatalf("layout %d: accepted %q, which is under the root", i, got)
			}
		}
	}
}

// repositoryWithSubdirectory returns a repository top marked by marker (".git"
// as a directory, ".git" as a worktree's file, or ".devcouncil"), its backend
// subdirectory, and a planted executable at tools/bin under the top.
func repositoryWithSubdirectory(t *testing.T, marker string, markerIsFile bool) (top, sub, planted string) {
	t.Helper()
	top = filepath.Join(t.TempDir(), "repo")
	sub = filepath.Join(top, "backend")
	if err := os.MkdirAll(sub, 0o755); err != nil {
		t.Fatal(err)
	}
	if markerIsFile {
		if err := os.WriteFile(filepath.Join(top, marker), []byte("gitdir: /elsewhere\n"), 0o644); err != nil {
			t.Fatal(err)
		}
	} else if err := os.Mkdir(filepath.Join(top, marker), 0o755); err != nil {
		t.Fatal(err)
	}
	planted = filepath.Join(top, "tools", "bin", "tool-under-test")
	writeExecutable(t, planted)
	return top, sub, planted
}

// Running from a subdirectory of the repository does not move the boundary to
// that subdirectory: the repository is the whole checkout, so a program the
// checkout carries beside it is still repository content. Root is given both
// explicitly and as the working directory a command without --project-root
// runs in.
func TestLookPathOutsideRefusesRepositoryContentAboveASubdirectoryRoot(t *testing.T) {
	for _, tc := range []struct {
		name         string
		marker       string
		markerIsFile bool
	}{
		{"git directory", ".git", false},
		{"worktree git file", ".git", true},
		{"devcouncil without git", ".devcouncil", false},
	} {
		t.Run(tc.name, func(t *testing.T) {
			_, sub, planted := repositoryWithSubdirectory(t, tc.marker, tc.markerIsFile)
			t.Setenv("PATH", filepath.Dir(planted))
			t.Chdir(sub)
			for _, root := range []string{sub, ""} {
				got, err := LookPathOutside("tool-under-test", root)
				if !errors.Is(err, ErrRepositoryLocal) {
					t.Fatalf("root %q: accepted repository content above the root: %q (err %v)", root, got, err)
				}
				got, err = ResolveComponent("tool-under-test", root, Override{})
				if !errors.Is(err, ErrRepositoryLocal) {
					t.Fatalf("root %q: ResolveComponent accepted repository content above the root: %q (err %v)", root, got, err)
				}
			}
		})
	}
}

// A .devcouncil below the nearest .git lies inside that checkout, so the
// boundary is the outer of the two.
func TestLookPathOutsideBoundaryIsTheOuterOfGitAndDevCouncil(t *testing.T) {
	top, sub, planted := repositoryWithSubdirectory(t, ".git", false)
	if err := os.Mkdir(filepath.Join(sub, ".devcouncil"), 0o755); err != nil {
		t.Fatal(err)
	}
	inner := filepath.Join(sub, "pkg")
	if err := os.Mkdir(inner, 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", filepath.Dir(planted))

	got, err := LookPathOutside("tool-under-test", inner)
	if !errors.Is(err, ErrRepositoryLocal) {
		t.Fatalf("a nested .devcouncil narrowed the boundary below %s: %q (err %v)", top, got, err)
	}
}

// homeWithDotfilesRepository builds a home directory that is itself a git
// repository (a dotfiles checkout), holding an installed component in
// ~/.local/bin, and an empty PATH.
func homeWithDotfilesRepository(t *testing.T) (home, installed string) {
	t.Helper()
	base := t.TempDir()
	home = filepath.Join(base, "home")
	for _, dir := range []string{filepath.Join(home, ".git"), filepath.Join(base, "empty")} {
		if err := os.MkdirAll(dir, 0o755); err != nil {
			t.Fatal(err)
		}
	}
	installed = filepath.Join(home, ".local", "bin", "tool-under-test")
	writeExecutable(t, installed)
	t.Setenv("PATH", filepath.Join(base, "empty"))
	t.Setenv("HOME", home)
	return home, installed
}

// The walk stops at the nearest .git. A repository inside a home directory
// that is itself a dotfiles repository is the inner checkout, so the
// installation in ~/.local/bin — outside it — still resolves, and a
// .devcouncil above that .git does not widen the boundary past it either.
func TestResolveComponentBoundaryStopsAtTheNearestGit(t *testing.T) {
	home, installed := homeWithDotfilesRepository(t)
	if err := os.Mkdir(filepath.Join(home, ".devcouncil"), 0o755); err != nil {
		t.Fatal(err)
	}
	repo := filepath.Join(home, "Code", "repo")
	sub := filepath.Join(repo, "backend")
	for _, dir := range []string{filepath.Join(repo, ".git"), sub} {
		if err := os.MkdirAll(dir, 0o755); err != nil {
			t.Fatal(err)
		}
	}
	want, err := filepath.EvalSymlinks(installed)
	if err != nil {
		t.Fatal(err)
	}
	for _, root := range []string{repo, sub} {
		got, err := ResolveComponent("tool-under-test", root, Override{})
		if err != nil {
			t.Fatalf("root %s: an outer repository swallowed ~/.local/bin: %v", root, err)
		}
		if got != want {
			t.Fatalf("root %s: resolved %q, want %q", root, got, want)
		}
	}
}

// An installation outside the repository still resolves when the root is a
// subdirectory of it.
func TestLookPathOutsideAcceptsInstallationOutsideFromASubdirectory(t *testing.T) {
	top, sub, _ := repositoryWithSubdirectory(t, ".git", false)
	installed := filepath.Join(filepath.Dir(top), "usr", "bin", "tool-under-test")
	writeExecutable(t, installed)
	t.Setenv("PATH", filepath.Dir(installed))
	t.Chdir(sub)

	want, err := filepath.EvalSymlinks(installed)
	if err != nil {
		t.Fatal(err)
	}
	for _, root := range []string{sub, ""} {
		got, err := LookPathOutside("tool-under-test", root)
		if err != nil {
			t.Fatalf("root %q: refused an installation outside the repository: %v", root, err)
		}
		if got != want {
			t.Fatalf("root %q: resolved %q, want %q", root, got, want)
		}
	}
}
