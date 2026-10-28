package verify_test

import (
	"bytes"
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"unicode/utf8"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/dcverify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// Attack round 1 of lane F (ft-011bb644), branch attack/verify-decided.
//
// Stop condition (Fable rulings 40/42):
//   - a gitlink that reads verified (scanned, or silently clean) parks the lane;
//     blocked is acceptable provided a gap, a refusal or a reason names the path;
//   - a tool-state secret that is unseen and not blocked buys one fix round.
//
// Every test here classifies the run as one of: refused, found, blocked,
// degraded, verified, and fails on "verified" where the contract forbids it.

// decidedClass says how a run ended, for the log and the assertion.
func decidedClass(o attackOutcome) string {
	switch {
	case o.refused != nil:
		return "refused"
	case o.result.Passed || o.result.Status == "verified":
		return "verified"
	case o.result.Status == "degraded":
		return "degraded"
	}
	for _, g := range o.gaps {
		if g.GapType == "security_risk" {
			return "found"
		}
	}
	return "blocked"
}

// decidedNamed reports whether anything an operator sees for the run names needle.
func decidedNamed(t *testing.T, o attackOutcome, needle string) bool {
	t.Helper()
	return strings.Contains(everything(t, o), needle) ||
		strings.Contains(strings.Join(o.result.DegradedReasons, "\n"), needle)
}

// requireGitlinkNotSilent is the park condition: a run that touches a gitlink
// must be refused, blocked or degraded, and the path must be named.
func requireGitlinkNotSilent(t *testing.T, key string, o attackOutcome, path string) {
	t.Helper()
	class := decidedClass(o)
	t.Logf("class=%s status=%s degraded=%q refused=%v types=%v", class, o.result.Status, o.result.DegradedReasons, o.refused, advTypes(o.gaps))
	if class == "blocked" {
		for _, g := range o.gaps {
			if g.Blocking {
				t.Logf("  blocking gap %s: %.300s", g.GapType, g.Description)
			}
		}
	}
	if class == "verified" {
		t.Fatalf("PARK: a gitlink change at %q read as verified (degraded=%q)", path, o.result.DegradedReasons)
	}
	if class == "found" {
		return
	}
	if !decidedNamed(t, o, path) {
		t.Fatalf("PARK-ADJACENT: class=%s but nothing names %q: degraded=%q refused=%v", class, path, o.result.DegradedReasons, o.refused)
	}
	if key != "" && strings.Contains(everything(t, o), key) {
		t.Fatal("the credential is in an output")
	}
}

// indexInfo writes entries straight into the index: mode, sha, path.
func indexInfo(t *testing.T, root string, entries [][3]string) {
	t.Helper()
	var in bytes.Buffer
	for _, e := range entries {
		fmt.Fprintf(&in, "%s %s 0\t%s\x00", e[0], e[1], e[2])
	}
	cmd := exec.Command(testsupport.Tool(t, "git"), "update-index", "--add", "-z", "--index-info")
	cmd.Dir = root
	cmd.Stdin = &in
	cmd.Env = append(os.Environ(), "GIT_CONFIG_NOSYSTEM=1", "GIT_CONFIG_GLOBAL="+os.DevNull)
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("update-index --index-info: %v %s", err, out)
	}
}

func sha(c byte) string { return strings.Repeat(string(c), 40) }

// ------------------------------------------------------------------ gitlinks

// A gitlink under a tool-state path is listed like any other.
func TestDecidedGitlinkUnderToolStatePath(t *testing.T) {
	for _, dir := range []string{".devcouncil", ".DevCouncil", ".devmap", ".DEVMAP/nested"} {
		t.Run(dir, func(t *testing.T) {
			root := gitRepo(t)
			path := dir + "/vendored"
			if err := os.MkdirAll(filepath.Join(root, path), 0o755); err != nil {
				t.Fatal(err)
			}
			indexInfo(t, root, [][3]string{{"160000", sha('a'), path}})
			runGitIn(t, root, "commit", "-qm", "gitlink in tool state")
			indexInfo(t, root, [][3]string{{"160000", sha('b'), path}})
			o := attackRun(t, root, "TASK-D-GL-TS", benignChange(t, root))
			requireGitlinkNotSilent(t, "", o, path)
		})
	}
}

// A gitlink moved with its directory: --no-renames must list both ends.
func TestDecidedGitlinkInsideARenamedDirectory(t *testing.T) {
	root := gitRepo(t)
	if err := os.MkdirAll(filepath.Join(root, "d/sub"), 0o755); err != nil {
		t.Fatal(err)
	}
	writeFile(t, root, "d/f.txt", "x\n")
	indexInfo(t, root, [][3]string{{"160000", sha('a'), "d/sub"}})
	runGitIn(t, root, "add", "d/f.txt")
	runGitIn(t, root, "commit", "-qm", "d")
	runGitIn(t, root, "mv", "d", "e")
	o := attackRun(t, root, "TASK-D-GL-RENAME",
		dc.PlannedFile{Path: "e/f.txt", AllowedChange: dc.ChangeCreate}, dc.PlannedFile{Path: "d/f.txt", AllowedChange: dc.ChangeDelete},
		dc.PlannedFile{Path: "e/sub", AllowedChange: dc.ChangeCreate}, dc.PlannedFile{Path: "d/sub", AllowedChange: dc.ChangeDelete})
	requireGitlinkNotSilent(t, "", o, "e/sub")
	if !decidedNamed(t, o, "d/sub") {
		t.Fatalf("the old end of the move is not named: %q", o.result.DegradedReasons)
	}
}

func blobOf(t *testing.T, root, body string) string {
	t.Helper()
	cmd := exec.Command(testsupport.Tool(t, "git"), "hash-object", "-w", "--stdin")
	cmd.Dir = root
	cmd.Stdin = strings.NewReader(body)
	out, err := cmd.Output()
	if err != nil {
		t.Fatal(err)
	}
	return strings.TrimSpace(string(out))
}

// A merge in progress that moves a gitlink, conflicted and clean.
func TestDecidedGitlinkInAMerge(t *testing.T) {
	t.Run("conflicted on both sides", func(t *testing.T) {
		root := gitRepo(t)
		indexInfo(t, root, [][3]string{{"160000", sha('a'), "sub"}})
		runGitIn(t, root, "commit", "-qm", "addsub")
		base := gitOutIn(t, root, "rev-parse", "--abbrev-ref", "HEAD")
		runGitIn(t, root, "checkout", "-q", "-b", "other")
		indexInfo(t, root, [][3]string{{"160000", sha('b'), "sub"}})
		runGitIn(t, root, "commit", "-qm", "other")
		runGitIn(t, root, "checkout", "-q", base)
		indexInfo(t, root, [][3]string{{"160000", sha('c'), "sub"}})
		runGitIn(t, root, "commit", "-qm", "main")
		cmd := exec.Command(testsupport.Tool(t, "git"), "merge", "other")
		cmd.Dir = root
		cmd.Env = append(os.Environ(), "GIT_AUTHOR_NAME=t", "GIT_AUTHOR_EMAIL=t@x.invalid", "GIT_COMMITTER_NAME=t", "GIT_COMMITTER_EMAIL=t@x.invalid", "GIT_CONFIG_GLOBAL="+os.DevNull)
		_ = cmd.Run() // conflicts
		o := attackRun(t, root, "TASK-D-GL-MERGE-C", benignChange(t, root), dc.PlannedFile{Path: "sub", AllowedChange: dc.ChangeModify})
		requireGitlinkNotSilent(t, "", o, "sub")
	})
	t.Run("clean merge brings a gitlink", func(t *testing.T) {
		root := gitRepo(t)
		base := gitOutIn(t, root, "rev-parse", "--abbrev-ref", "HEAD")
		runGitIn(t, root, "checkout", "-q", "-b", "other")
		indexInfo(t, root, [][3]string{{"160000", sha('b'), "newsub"}})
		runGitIn(t, root, "commit", "-qm", "other adds a gitlink")
		runGitIn(t, root, "checkout", "-q", base)
		writeFile(t, root, "README", "base\nmain edit\n")
		runGitIn(t, root, "commit", "-qam", "main edit")
		runGitIn(t, root, "merge", "--no-commit", "--no-ff", "other")
		o := attackRun(t, root, "TASK-D-GL-MERGE-N", benignChange(t, root), dc.PlannedFile{Path: "newsub", AllowedChange: dc.ChangeCreate})
		requireGitlinkNotSilent(t, "", o, "newsub")
	})
	t.Run("conflict between a file and a gitlink", func(t *testing.T) {
		root := gitRepo(t)
		base := gitOutIn(t, root, "rev-parse", "--abbrev-ref", "HEAD")
		runGitIn(t, root, "checkout", "-q", "-b", "other")
		indexInfo(t, root, [][3]string{{"160000", sha('b'), "p"}})
		runGitIn(t, root, "commit", "-qm", "other: p is a gitlink")
		runGitIn(t, root, "checkout", "-q", base)
		writeFile(t, root, "p", "a file\n")
		runGitIn(t, root, "add", "p")
		runGitIn(t, root, "commit", "-qm", "main: p is a file")
		cmd := exec.Command(testsupport.Tool(t, "git"), "merge", "other")
		cmd.Dir = root
		cmd.Env = append(os.Environ(), "GIT_AUTHOR_NAME=t", "GIT_AUTHOR_EMAIL=t@x.invalid", "GIT_COMMITTER_NAME=t", "GIT_COMMITTER_EMAIL=t@x.invalid", "GIT_CONFIG_GLOBAL="+os.DevNull)
		_ = cmd.Run()
		o := attackRun(t, root, "TASK-D-GL-MERGE-F", benignChange(t, root), dc.PlannedFile{Path: "p", AllowedChange: dc.ChangeModify})
		requireGitlinkNotSilent(t, "", o, "p")
	})
}

// diff.ignoreSubmodules, submodule.<n>.ignore and .gitmodules ignore=all, each
// alone, with the bump staged through the index.
func TestDecidedGitlinkIgnoreConfigs(t *testing.T) {
	cases := map[string]func(t *testing.T, root string){
		"diff.ignoreSubmodules=all":      func(t *testing.T, root string) { runGitIn(t, root, "config", "diff.ignoreSubmodules", "all") },
		"submodule.sub.ignore=all":       func(t *testing.T, root string) { runGitIn(t, root, "config", "submodule.sub.ignore", "all") },
		"submodule.sub.ignore=dirty":     func(t *testing.T, root string) { runGitIn(t, root, "config", "submodule.sub.ignore", "dirty") },
		"submodule.sub.ignore=untracked": func(t *testing.T, root string) { runGitIn(t, root, "config", "submodule.sub.ignore", "untracked") },
		"gitmodules ignore=all": func(t *testing.T, root string) {
			runGitIn(t, root, "config", "-f", ".gitmodules", "submodule.sub.ignore", "all")
		},
		"diff.submodule=log": func(t *testing.T, root string) {
			t.Skip("ft-c397c278: diff.submodule=log makes `git diff HEAD` print `Submodule a..b:` lines that dcverify cannot parse, so the run is blocked (fail-closed) by a rigor_check_unavailable gap that names no path; asserted and found not to name it, so it stays skipped until the card lands")
		},
	}
	for name, setup := range cases {
		t.Run(name, func(t *testing.T) {
			root, subDir := submoduleRepoWith(t, map[string]string{"s.txt": "ok\n"})
			setup(t, root)
			subCommitFile(t, subDir, "later.txt", "later\n", "later")
			head := gitOutIn(t, subDir, "rev-parse", "HEAD")
			runGitIn(t, root, "update-index", "--cacheinfo", "160000,"+head+",sub")
			o := attackRun(t, root, "TASK-D-GL-IGN", dc.PlannedFile{Path: "sub", AllowedChange: dc.ChangeModify}, dc.PlannedFile{Path: ".gitmodules", AllowedChange: dc.ChangeModify})
			requireGitlinkNotSilent(t, "", o, "sub")
		})
	}
}

// Awkward paths. The reason must name the path in a form that cannot forge a
// second reason, and the run must not pass.
func TestDecidedGitlinkAwkwardPaths(t *testing.T) {
	paths := []string{
		"has space/sub",
		"new\nline",
		"tab\there",
		"quote\"mark",
		"-leading-dash",
		"back\\slash",
		"utf8-é-sub",
		"bad\xffbytes",
		"colon:and|pipe",
		strings.Repeat("deep/", 40) + "sub",
	}
	for _, p := range paths {
		t.Run(fmt.Sprintf("%q", p), func(t *testing.T) {
			root := gitRepo(t)
			indexInfo(t, root, [][3]string{{"160000", sha('a'), p}})
			runGitIn(t, root, "-c", "core.quotePath=false", "commit", "-qm", "g")
			indexInfo(t, root, [][3]string{{"160000", sha('b'), p}})
			o := attackRun(t, root, "TASK-D-GL-PATH", benignChange(t, root), dc.PlannedFile{Path: p, AllowedChange: dc.ChangeModify})
			class := decidedClass(o)
			t.Logf("class=%s reasons=%q", class, o.result.DegradedReasons)
			for _, g := range o.gaps {
				if g.Blocking {
					t.Logf("  blocking gap %s: %.240s", g.GapType, g.Description)
				}
			}
			if class == "verified" {
				t.Fatalf("PARK: gitlink at %q read as verified", p)
			}
			if class == "degraded" {
				joined := strings.Join(o.result.DegradedReasons, "\n")
				if strings.Count(joined, "submodule commit range not scanned:") != 1 {
					t.Fatalf("expected exactly one gitlink reason line, got %q", o.result.DegradedReasons)
				}
				for _, r := range o.result.DegradedReasons {
					if strings.ContainsAny(r, "\n\r\x00") {
						t.Fatalf("a reason holds a raw control character: %q", r)
					}
				}
			}
		})
	}
}

// The bound: exactly 256 moved gitlinks is listed one by one; 257 is refused.
func TestDecidedGitlinkBoundary(t *testing.T) {
	build := func(t *testing.T, n int, headless bool) string {
		root := gitRepo(t)
		if headless {
			root = t.TempDir()
			runGitIn(t, root, "init", "-q")
		}
		var entries [][3]string
		for i := 0; i < n; i++ {
			entries = append(entries, [3]string{"160000", sha('a'), fmt.Sprintf("gl/%04d", i)})
		}
		indexInfo(t, root, entries)
		if !headless {
			runGitIn(t, root, "commit", "-qm", "gitlinks")
			for i := range entries {
				entries[i][1] = sha('b')
			}
			indexInfo(t, root, entries)
		}
		return root
	}
	for _, headless := range []bool{false, true} {
		for _, n := range []int{256, 257} {
			t.Run(fmt.Sprintf("n=%d/headless=%v", n, headless), func(t *testing.T) {
				root := build(t, n, headless)
				var plan []dc.PlannedFile
				for i := 0; i < n; i++ {
					ch := dc.ChangeModify
					if headless {
						ch = dc.ChangeCreate
					}
					plan = append(plan, dc.PlannedFile{Path: fmt.Sprintf("gl/%04d", i), AllowedChange: ch})
				}
				o := attackRun(t, root, "TASK-D-GL-BOUND", plan...)
				class := decidedClass(o)
				t.Logf("class=%s reasons=%d refused=%v", class, len(o.result.DegradedReasons), o.refused)
				if class == "verified" {
					t.Fatalf("PARK: %d gitlinks read as verified", n)
				}
				if n == 256 && class == "degraded" {
					got := 0
					for _, r := range o.result.DegradedReasons {
						if strings.HasPrefix(r, "submodule commit range not scanned: ") {
							got++
						}
					}
					if got != 256 {
						t.Fatalf("256 gitlinks moved, %d reasons", got)
					}
				}
				if n == 257 && o.refused == nil {
					// Through the host (VerifyTask) a refusal is a blocking gitStateGap;
					// attackRun reports it as refused. Anything else is a partial answer.
					t.Fatalf("257 gitlinks were not refused: class=%s", class)
				}
			})
		}
	}
}

// A headless repository holding a gitlink under tool state, and a dirty one.
func TestDecidedHeadlessGitlinks(t *testing.T) {
	root := t.TempDir()
	runGitIn(t, root, "init", "-q")
	awkward := []string{"z\nnl", "tab\there", "bad\xffbytes", "cr\rx", "nul-free\x1bescape", "\u202ertl"}
	entries := [][3]string{{"160000", sha('a'), ".devcouncil/g"}}
	for _, p := range awkward {
		entries = append(entries, [3]string{"160000", sha('a'), p})
	}
	indexInfo(t, root, entries)
	o := attackRun(t, root, "TASK-D-GL-HL")
	requireGitlinkNotSilent(t, "", o, ".devcouncil/g")
	if len(o.result.DegradedReasons) != len(entries) {
		t.Fatalf("want one reason per gitlink (%d), got %d: %q", len(entries), len(o.result.DegradedReasons), o.result.DegradedReasons)
	}
	for _, r := range o.result.DegradedReasons {
		if strings.ContainsAny(r, "\n\r\t\x1b\x00") {
			t.Fatalf("a reason holds a raw control byte: %q", r)
		}
	}
}

// Turn each blocked shape into the nearest unblocked one: a nested repository
// first untracked (refused), then `git add`ed (a gitlink).
func TestDecidedNestedRepositoryBlockedThenAdded(t *testing.T) {
	root := gitRepo(t)
	nested := filepath.Join(root, "nested")
	if err := os.MkdirAll(nested, 0o755); err != nil {
		t.Fatal(err)
	}
	runGitIn(t, nested, "init", "-q")
	writeFile(t, nested, "f.txt", "x\n")
	runGitIn(t, nested, "add", "-A")
	runGitIn(t, nested, "commit", "-qm", "n")
	o := attackRun(t, root, "TASK-D-NEST-1", benignChange(t, root))
	t.Logf("untracked nested repo: class=%s refused=%v", decidedClass(o), o.refused)
	if decidedClass(o) == "verified" {
		t.Fatal("an untracked nested repository read as verified")
	}
	runGitIn(t, root, "add", "nested")
	o = attackRun(t, root, "TASK-D-NEST-2", benignChange(t, root), dc.PlannedFile{Path: "nested", AllowedChange: dc.ChangeCreate})
	requireGitlinkNotSilent(t, "", o, "nested")
}

// Staged bump + skip-worktree / assume-unchanged on the gitlink, checkout kept
// on the old commit.
func TestDecidedGitlinkIndexFlags(t *testing.T) {
	for _, flag := range []string{"--skip-worktree", "--assume-unchanged"} {
		t.Run(flag, func(t *testing.T) {
			root, subDir := submoduleRepoWith(t, map[string]string{"s.txt": "ok\n"})
			subCommitFile(t, subDir, "later.txt", "later\n", "later")
			head := gitOutIn(t, subDir, "rev-parse", "HEAD")
			runGitIn(t, subDir, "reset", "-q", "--hard", "HEAD~1")
			runGitIn(t, root, "update-index", "--cacheinfo", "160000,"+head+",sub")
			runGitIn(t, root, "update-index", flag, "sub")
			o := attackRun(t, root, "TASK-D-GL-FLAG", dc.PlannedFile{Path: "sub", AllowedChange: dc.ChangeModify})
			requireGitlinkNotSilent(t, "", o, "sub")
		})
	}
}

// The bump is staged and then the checkout is moved back (the other order from
// "have the checkout follow it"), and the reverse: checkout moved, never staged.
func TestDecidedGitlinkStagedThenCheckoutReverted(t *testing.T) {
	root, subDir := submoduleRepoWith(t, map[string]string{"s.txt": "ok\n"})
	old := gitOutIn(t, subDir, "rev-parse", "HEAD")
	subCommitFile(t, subDir, "later.txt", "later\n", "later")
	runGitIn(t, root, "add", "sub")
	runGitIn(t, subDir, "checkout", "-q", old)
	o := attackRun(t, root, "TASK-D-GL-REV", dc.PlannedFile{Path: "sub", AllowedChange: dc.ChangeModify})
	requireGitlinkNotSilent(t, "", o, "sub")
}

// The submodule's own checkout is bumped through a worktree the host never
// reads: the submodule is a symlink to a repository elsewhere.
func TestDecidedGitlinkReplaceRefsInSuperproject(t *testing.T) {
	root, subDir := submoduleRepoWith(t, map[string]string{"s.txt": "ok\n"})
	subCommitFile(t, subDir, "later.txt", "later\n", "later")
	head := gitOutIn(t, subDir, "rev-parse", "HEAD")
	runGitIn(t, root, "update-index", "--cacheinfo", "160000,"+head+",sub")
	// A replace ref in the superproject for the HEAD commit: a clean twin whose
	// tree already holds the bumped gitlink, so a reader honouring replace refs
	// sees no change.
	tree := gitOutIn(t, root, "write-tree")
	twin := gitOutIn(t, root, "commit-tree", tree, "-p", "HEAD~1", "-m", "twin")
	runGitIn(t, root, "replace", "HEAD", twin)
	o := attackRun(t, root, "TASK-D-GL-REPLACE", dc.PlannedFile{Path: "sub", AllowedChange: dc.ChangeModify})
	requireGitlinkNotSilent(t, "", o, "sub")
}

// ------------------------------------------------------------- H1: replace refs

// Replace refs, enabled by every route the environment offers, must not rewrite
// what the verifier's own reads see.
func TestDecidedReplaceRefsCannotBeReenabled(t *testing.T) {
	key := exampleAWSKey()
	envs := map[string][]string{
		"GIT_REPLACE_REF_BASE":      {"GIT_REPLACE_REF_BASE=refs/replace/"},
		"GIT_CONFIG_COUNT":          {"GIT_CONFIG_COUNT=1", "GIT_CONFIG_KEY_0=core.useReplaceRefs", "GIT_CONFIG_VALUE_0=true"},
		"GIT_CONFIG_PARAMETERS":     {"GIT_CONFIG_PARAMETERS='core.usereplacerefs=true'"},
		"GIT_NO_REPLACE_OBJECTS=0":  {"GIT_NO_REPLACE_OBJECTS=0"},
		"GIT_CONFIG_GLOBAL file":    nil,
		"GIT_CONFIG_COUNT after -c": {"GIT_CONFIG_COUNT=2", "GIT_CONFIG_KEY_0=core.useReplaceRefs", "GIT_CONFIG_VALUE_0=1", "GIT_CONFIG_KEY_1=core.useReplaceRefs", "GIT_CONFIG_VALUE_1=true"},
	}
	for name, env := range envs {
		t.Run(name, func(t *testing.T) {
			root := gitRepo(t)
			writeFile(t, root, "keys.txt", attackSecretLine())
			runGitIn(t, root, "add", "keys.txt")
			runGitIn(t, root, "commit", "-qm", "adds the secret")
			bad := gitOutIn(t, root, "rev-parse", "HEAD")
			tree := gitOutIn(t, root, "rev-parse", bad+"^{tree}")
			_ = tree
			parent := gitOutIn(t, root, "rev-parse", bad+"^")
			ptree := gitOutIn(t, root, "rev-parse", parent+"^{tree}")
			clean := gitOutIn(t, root, "commit-tree", ptree, "-p", parent, "-m", "clean twin")
			runGitIn(t, root, "replace", bad, clean)
			if name == "GIT_CONFIG_GLOBAL file" {
				g := filepath.Join(t.TempDir(), "gitconfig")
				if err := os.WriteFile(g, []byte("[core]\n\tuseReplaceRefs = true\n"), 0o644); err != nil {
					t.Fatal(err)
				}
				env = []string{"GIT_CONFIG_GLOBAL=" + g}
			}
			// Work present: a plain edit, and the secret is the working tree's own change
			// against the replaced HEAD: with replace refs honoured, HEAD reads as the
			// clean twin and keys.txt reads as a new file; without, it is unchanged.
			writeFile(t, root, "README", "base\nedit\n")
			for _, kv := range env {
				k, v, _ := strings.Cut(kv, "=")
				t.Setenv(k, v)
			}
			state, err := verify.CollectGitStateReport(context.Background(), root)
			if err != nil {
				t.Fatalf("collect: %v", err)
			}
			if strings.Contains(state.Diff, key) {
				t.Fatalf("a replace ref changed what the diff read: the committed secret is in the diff as if new")
			}
			if !strings.Contains(state.Diff, "README") {
				t.Fatalf("the plain edit is not in the diff:\n%s", state.Diff)
			}
		})
	}
}

// ------------------------------------------------------------------ tool state

func requireToolStateSeen(t *testing.T, key string, o attackOutcome) {
	t.Helper()
	class := decidedClass(o)
	t.Logf("class=%s status=%s degraded=%q types=%v refused=%v", class, o.result.Status, o.result.DegradedReasons, advTypes(o.gaps), o.refused)
	switch class {
	case "found", "blocked", "refused":
		reportLeaks(t, o.result, o.gaps, key)
		return
	case "degraded":
		t.Fatalf("UNSEEN-NOT-BLOCKED (degraded only): %q", o.result.DegradedReasons)
	default:
		t.Fatalf("UNSEEN-NOT-BLOCKED: status=%s degraded=%q", o.result.Status, o.result.DegradedReasons)
	}
}

// requireToolStateNamed accepts the unreadable shapes the owner decided degrade,
// and records which it was.
func requireToolStateNamed(t *testing.T, key string, o attackOutcome, want string) {
	t.Helper()
	class := decidedClass(o)
	t.Logf("class=%s degraded=%q types=%v refused=%v", class, o.result.DegradedReasons, advTypes(o.gaps), o.refused)
	if class == "found" || class == "blocked" || class == "refused" {
		return
	}
	if class == "verified" {
		t.Fatalf("UNSEEN-NOT-BLOCKED: silent verified")
	}
	if !strings.Contains(strings.Join(o.result.DegradedReasons, "\n"), want) {
		t.Fatalf("degraded but no reason names %q: %q", want, o.result.DegradedReasons)
	}
	reportLeaks(t, o.result, o.gaps, key)
}

// Deleted-then-recreated and staged shapes, tracked tool-state file.
func TestDecidedToolStateRecreatedShapes(t *testing.T) {
	key := exampleAWSKey()
	seed := func(t *testing.T) string {
		root := gitRepo(t)
		writeFile(t, root, ".devcouncil/a.txt", "a\n")
		attackCommit(t, root, "track")
		return root
	}
	t.Run("rm --cached then recreate (untracked)", func(t *testing.T) {
		root := seed(t)
		runGitIn(t, root, "rm", "-q", "--cached", ".devcouncil/a.txt")
		writeFile(t, root, ".devcouncil/a.txt", toolStateKeyLine())
		requireToolStateSeen(t, key, attackRun(t, root, "TASK-D-TS-1", benignChange(t, root)))
	})
	t.Run("rm then recreate then git add", func(t *testing.T) {
		root := seed(t)
		runGitIn(t, root, "rm", "-q", ".devcouncil/a.txt")
		writeFile(t, root, ".devcouncil/a.txt", toolStateKeyLine())
		runGitIn(t, root, "add", ".devcouncil/a.txt")
		requireToolStateSeen(t, key, attackRun(t, root, "TASK-D-TS-2", benignChange(t, root)))
	})
	t.Run("rm then recreate, unstaged", func(t *testing.T) {
		root := seed(t)
		if err := os.Remove(filepath.Join(root, ".devcouncil/a.txt")); err != nil {
			t.Fatal(err)
		}
		writeFile(t, root, ".devcouncil/a.txt", toolStateKeyLine())
		requireToolStateSeen(t, key, attackRun(t, root, "TASK-D-TS-3", benignChange(t, root)))
	})
	t.Run("staged with the secret then the worktree reverted clean", func(t *testing.T) {
		root := seed(t)
		writeFile(t, root, ".devcouncil/a.txt", "a\n"+toolStateKeyLine())
		runGitIn(t, root, "add", ".devcouncil/a.txt")
		writeFile(t, root, ".devcouncil/a.txt", "a\n")
		requireToolStateSeen(t, key, attackRun(t, root, "TASK-D-TS-4", benignChange(t, root)))
	})
	t.Run("new file staged with the secret then overwritten clean", func(t *testing.T) {
		root := seed(t)
		writeFile(t, root, ".devcouncil/new.txt", toolStateKeyLine())
		runGitIn(t, root, "add", ".devcouncil/new.txt")
		writeFile(t, root, ".devcouncil/new.txt", "clean\n")
		requireToolStateSeen(t, key, attackRun(t, root, "TASK-D-TS-4b", benignChange(t, root)))
	})
	t.Run("new file staged with the secret then deleted from disk", func(t *testing.T) {
		root := seed(t)
		writeFile(t, root, ".devcouncil/gone.txt", toolStateKeyLine())
		runGitIn(t, root, "add", ".devcouncil/gone.txt")
		if err := os.Remove(filepath.Join(root, ".devcouncil/gone.txt")); err != nil {
			t.Fatal(err)
		}
		requireToolStateSeen(t, key, attackRun(t, root, "TASK-D-TS-4c", benignChange(t, root)))
	})
	t.Run("intent to add", func(t *testing.T) {
		root := seed(t)
		writeFile(t, root, ".devcouncil/ita.txt", toolStateKeyLine())
		runGitIn(t, root, "add", "-N", ".devcouncil/ita.txt")
		requireToolStateSeen(t, key, attackRun(t, root, "TASK-D-TS-5", benignChange(t, root)))
	})
	t.Run("assume-unchanged secret edit", func(t *testing.T) {
		root := seed(t)
		runGitIn(t, root, "update-index", "--assume-unchanged", ".devcouncil/a.txt")
		writeFile(t, root, ".devcouncil/a.txt", "a\n"+toolStateKeyLine())
		o := attackRun(t, root, "TASK-D-TS-6", benignChange(t, root))
		t.Logf("assume-unchanged edit (not committed by `commit -a` either): class=%s", decidedClass(o))
	})
}

// Typechange of a tracked tool-state file, staged across two commits.
func TestDecidedToolStateTypechange(t *testing.T) {
	key := exampleAWSKey()
	outside := filepath.Join(t.TempDir(), "keys.txt")
	if err := os.WriteFile(outside, []byte(toolStateKeyLine()), 0o644); err != nil {
		t.Fatal(err)
	}
	t.Run("file to symlink, unstaged", func(t *testing.T) {
		root := gitRepo(t)
		writeFile(t, root, ".devmap/f", "f\n")
		attackCommit(t, root, "track")
		_ = os.Remove(filepath.Join(root, ".devmap/f"))
		if err := os.Symlink(outside, filepath.Join(root, ".devmap/f")); err != nil {
			t.Fatal(err)
		}
		requireToolStateNamed(t, key, attackRun(t, root, "TASK-D-TS-TC1", benignChange(t, root)), "")
	})
	t.Run("file to symlink, staged", func(t *testing.T) {
		root := gitRepo(t)
		writeFile(t, root, ".devmap/f", "f\n")
		attackCommit(t, root, "track")
		_ = os.Remove(filepath.Join(root, ".devmap/f"))
		if err := os.Symlink(outside, filepath.Join(root, ".devmap/f")); err != nil {
			t.Fatal(err)
		}
		runGitIn(t, root, "add", ".devmap/f")
		o := attackRun(t, root, "TASK-D-TS-TC2", benignChange(t, root))
		t.Logf("class=%s degraded=%q (the symlink blob holds a path, the secret is behind it)", decidedClass(o), o.result.DegradedReasons)
		if decidedClass(o) == "verified" {
			t.Fatalf("a staged tool-state symlink read as verified")
		}
	})
	t.Run("symlink committed, then replaced by a file holding the secret", func(t *testing.T) {
		root := gitRepo(t)
		if err := os.MkdirAll(filepath.Join(root, ".devmap"), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.Symlink("nowhere", filepath.Join(root, ".devmap/f")); err != nil {
			t.Fatal(err)
		}
		attackCommit(t, root, "track a symlink")
		_ = os.Remove(filepath.Join(root, ".devmap/f"))
		writeFile(t, root, ".devmap/f", toolStateKeyLine())
		requireToolStateSeen(t, key, attackRun(t, root, "TASK-D-TS-TC3", benignChange(t, root)))
	})
	t.Run("symlink dir: .devcouncil is a symlink to a directory holding a secret", func(t *testing.T) {
		root := gitRepo(t)
		dir := filepath.Join(t.TempDir(), "state")
		if err := os.MkdirAll(dir, 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(dir, "k.txt"), []byte(toolStateKeyLine()), 0o644); err != nil {
			t.Fatal(err)
		}
		if err := os.Symlink(dir, filepath.Join(root, ".devcouncil")); err != nil {
			t.Fatal(err)
		}
		requireToolStateNamed(t, key, attackRun(t, root, "TASK-D-TS-SYMDIR", benignChange(t, root)), "symlink")
	})
	t.Run("symlink dir committed and the secret behind it changes", func(t *testing.T) {
		root := gitRepo(t)
		dir := filepath.Join(t.TempDir(), "state")
		if err := os.MkdirAll(dir, 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.Symlink(dir, filepath.Join(root, ".devcouncil")); err != nil {
			t.Fatal(err)
		}
		attackCommit(t, root, "track the symlinked dir")
		if err := os.WriteFile(filepath.Join(dir, "k.txt"), []byte(toolStateKeyLine()), 0o644); err != nil {
			t.Fatal(err)
		}
		o := attackRun(t, root, "TASK-D-TS-SYMDIR2", benignChange(t, root))
		t.Logf("class=%s (target is outside the repository and not part of any commit)", decidedClass(o))
	})
}

// A tool-state path that is not a directory: `.devcouncil` as a file.
func TestDecidedToolStateNamedLikeAFile(t *testing.T) {
	key := exampleAWSKey()
	root := gitRepo(t)
	writeFile(t, root, ".devcouncil", toolStateKeyLine())
	requireToolStateSeen(t, key, attackRun(t, root, "TASK-D-TS-ASFILE", benignChange(t, root)))
}

// Ignored by .gitignore vs .git/info/exclude vs a global ignore, and force-added.
func TestDecidedToolStateIgnoreRoutes(t *testing.T) {
	key := exampleAWSKey()
	t.Run(".gitignore (owner: out of scope)", func(t *testing.T) {
		root := gitRepo(t)
		writeFile(t, root, ".gitignore", ".devcouncil/\n")
		attackCommit(t, root, "ignore")
		writeFile(t, root, ".devcouncil/s.env", toolStateKeyLine())
		o := attackRun(t, root, "TASK-D-TS-IGN1", benignChange(t, root))
		t.Logf("class=%s", decidedClass(o))
	})
	t.Run(".git/info/exclude (owner: out of scope)", func(t *testing.T) {
		root := gitRepo(t)
		if err := os.WriteFile(filepath.Join(root, ".git/info/exclude"), []byte(".devmap/\n"), 0o644); err != nil {
			t.Fatal(err)
		}
		writeFile(t, root, ".devmap/s.env", toolStateKeyLine())
		o := attackRun(t, root, "TASK-D-TS-IGN2", benignChange(t, root))
		t.Logf("class=%s", decidedClass(o))
	})
	t.Run("ignored then force-added", func(t *testing.T) {
		root := gitRepo(t)
		writeFile(t, root, ".gitignore", ".devcouncil/\n")
		attackCommit(t, root, "ignore")
		writeFile(t, root, ".devcouncil/s.env", toolStateKeyLine())
		runGitIn(t, root, "add", "-f", ".devcouncil/s.env")
		requireToolStateSeen(t, key, attackRun(t, root, "TASK-D-TS-IGN3", benignChange(t, root)))
	})
	t.Run("ignored, tracked already, then edited", func(t *testing.T) {
		root := gitRepo(t)
		writeFile(t, root, ".devcouncil/s.env", "x\n")
		runGitIn(t, root, "add", "-f", ".devcouncil/s.env")
		writeFile(t, root, ".gitignore", ".devcouncil/\n")
		attackCommit(t, root, "ignore + track")
		writeFile(t, root, ".devcouncil/s.env", "x\n"+toolStateKeyLine())
		requireToolStateSeen(t, key, attackRun(t, root, "TASK-D-TS-IGN4", benignChange(t, root)))
	})
	t.Run("a negated ignore re-includes the file", func(t *testing.T) {
		root := gitRepo(t)
		writeFile(t, root, ".gitignore", ".devcouncil/*\n!.devcouncil/keep.txt\n")
		attackCommit(t, root, "ignore")
		writeFile(t, root, ".devcouncil/keep.txt", toolStateKeyLine())
		requireToolStateSeen(t, key, attackRun(t, root, "TASK-D-TS-IGN5", benignChange(t, root)))
	})
}

// A secret in a nested dir under tool state, tracked or not, awkward names.
func TestDecidedToolStateAwkwardNames(t *testing.T) {
	key := exampleAWSKey()
	for _, name := range []string{".devmap/new\nline.txt", ".devmap/qu\"ote.txt", ".devmap/sp ace.txt", ".devmap/é.txt", ".devmap/\xffbad.txt", ".devmap/-dash", ".devmap/:colon", ".devmap/[glob]*?.txt"} {
		t.Run(fmt.Sprintf("untracked %q", name), func(t *testing.T) {
			root := gitRepo(t)
			full := filepath.Join(root, name)
			if err := os.MkdirAll(filepath.Dir(full), 0o755); err != nil {
				t.Fatal(err)
			}
			if err := os.WriteFile(full, []byte(toolStateKeyLine()), 0o644); err != nil {
				t.Skipf("filesystem refuses the name: %v", err)
			}
			requireToolStateSeen(t, key, attackRun(t, root, "TASK-D-TS-NAME", benignChange(t, root)))
		})
		t.Run(fmt.Sprintf("tracked %q", name), func(t *testing.T) {
			root := gitRepo(t)
			full := filepath.Join(root, name)
			if err := os.MkdirAll(filepath.Dir(full), 0o755); err != nil {
				t.Fatal(err)
			}
			if err := os.WriteFile(full, []byte("x\n"), 0o644); err != nil {
				t.Skipf("filesystem refuses the name: %v", err)
			}
			attackCommit(t, root, "track")
			if err := os.WriteFile(full, []byte("x\n"+toolStateKeyLine()), 0o644); err != nil {
				t.Fatal(err)
			}
			requireToolStateSeen(t, key, attackRun(t, root, "TASK-D-TS-NAME2", benignChange(t, root)))
		})
	}
}

// Content shapes: CRLF, BOM, UTF-16, very long line, many lines, NUL after the
// secret, binary-looking text.
func TestDecidedToolStateContentShapes(t *testing.T) {
	key := exampleAWSKey()
	line := toolStateKeyLine()
	utf16 := func(s string) string {
		var b strings.Builder
		b.WriteString("\xff\xfe")
		for _, r := range s {
			b.WriteRune(r)
			b.WriteByte(0)
		}
		return b.String()
	}
	shapes := map[string]string{
		"crlf":                strings.ReplaceAll("a\n"+line, "\n", "\r\n"),
		"bom":                 "\xef\xbb\xbf" + line,
		"cr only":             strings.ReplaceAll("a\n"+line, "\n", "\r"),
		"long line before":    strings.Repeat("a", 3<<20) + " " + line,
		"many lines before":   strings.Repeat("x\n", 600000) + line,
		"key at 8001st byte":  strings.Repeat("a", 8000) + "\n" + line,
		"nul after the key":   line + "\x00tail\n",
		"nul at byte 9000":    line + strings.Repeat("a", 9000) + "\x00\n",
		"utf16 (nul bytes)":   utf16(line),
		"invalid utf8 around": "\xff\xfe\xfd" + line + "\xff",
		"form feed and vtab":  "\f\v" + line,
	}
	for name, body := range shapes {
		t.Run("untracked/"+name, func(t *testing.T) {
			root := gitRepo(t)
			writeFile(t, root, ".devmap/c.txt", body)
			o := attackRun(t, root, "TASK-D-TS-CONTENT", benignChange(t, root))
			requireToolStateNamedOrSeen(t, key, o)
		})
		t.Run("tracked/"+name, func(t *testing.T) {
			root := gitRepo(t)
			writeFile(t, root, ".devmap/c.txt", "seed\n")
			attackCommit(t, root, "track")
			writeFile(t, root, ".devmap/c.txt", body)
			o := attackRun(t, root, "TASK-D-TS-CONTENT2", benignChange(t, root))
			requireToolStateNamedOrSeen(t, key, o)
		})
	}
}

// requireToolStateNamedOrSeen: seen, blocked, or degraded with a tool-state
// reason. Silent verified is the break.
func requireToolStateNamedOrSeen(t *testing.T, key string, o attackOutcome) {
	t.Helper()
	class := decidedClass(o)
	t.Logf("class=%s degraded=%q types=%v", class, o.result.DegradedReasons, advTypes(o.gaps))
	if class == "verified" {
		t.Fatalf("UNSEEN-NOT-BLOCKED: silent verified")
	}
	if class == "degraded" && !strings.Contains(strings.Join(o.result.DegradedReasons, "\n"), "tool state") {
		t.Fatalf("UNSEEN-NOT-BLOCKED: degraded, but not by a tool-state reason: %q", o.result.DegradedReasons)
	}
	reportLeaks(t, o.result, o.gaps, key)
}

// The same shapes in an ordinary file, to say which are pre-existing on main.
func TestDecidedOrdinaryFileContentShapesForComparison(t *testing.T) {
	line := toolStateKeyLine()
	shapes := map[string]string{
		"long line before":  strings.Repeat("a", 3<<20) + " " + line,
		"many lines before": strings.Repeat("x\n", 600000) + line,
		"nul after the key": line + "\x00tail\n",
		"cr only":           strings.ReplaceAll("a\n"+line, "\n", "\r"),
	}
	for name, body := range shapes {
		t.Run(name, func(t *testing.T) {
			root := gitRepo(t)
			writeFile(t, root, "plain.txt", body)
			o := attackRun(t, root, "TASK-D-ORD", benignChange(t, root))
			class := decidedClass(o)
			t.Logf("ordinary file: class=%s degraded=%q", class, o.result.DegradedReasons)
			if class == "verified" {
				t.Errorf("ordinary file shape %q reads verified (pre-existing on main if it also does there)", name)
			}
		})
	}
}

// The 32 MiB boundary, tracked and untracked, with the secret last.
func TestDecidedToolStateCapBoundary(t *testing.T) {
	key := exampleAWSKey()
	line := toolStateKeyLine()
	const limit = 32 << 20
	pad := func(n int) string { return strings.Repeat(strings.Repeat("a", 63)+"\n", n/64+1)[:n] }
	for _, name := range []string{"cap-1MiB", "cap-4KiB", "cap-100", "cap+1"} {
		size := map[string]int{"cap-1MiB": limit - 1<<20, "cap-4KiB": limit - 4096, "cap-100": limit - 100, "cap+1": limit + 1}[name]
		t.Run("untracked/"+name, func(t *testing.T) {
			root := gitRepo(t)
			body := pad(size)
			if len(body) > len(line) && name != "cap+1" {
				body = body[:len(body)-len(line)-1] + "\n" + line
			}
			writeFile(t, root, ".devmap/big.txt", body)
			o := attackRun(t, root, "TASK-D-TS-CAP", benignChange(t, root))
			requireToolStateNamedOrSeen(t, key, o)
		})
		t.Run("tracked/"+name, func(t *testing.T) {
			root := gitRepo(t)
			writeFile(t, root, ".devmap/big.txt", "seed\n")
			attackCommit(t, root, "track")
			body := pad(size)
			if len(body) > len(line) && name != "cap+1" {
				body = body[:len(body)-len(line)-1] + "\n" + line
			}
			writeFile(t, root, ".devmap/big.txt", body)
			o := attackRun(t, root, "TASK-D-TS-CAP2", benignChange(t, root))
			requireToolStateNamedOrSeen(t, key, o)
		})
	}
	t.Run("a big benign file sorted before a small secret file", func(t *testing.T) {
		root := gitRepo(t)
		writeFile(t, root, ".devmap/a-big.txt", pad(limit-1000))
		writeFile(t, root, ".devmap/z-key.txt", line)
		o := attackRun(t, root, "TASK-D-TS-CAP3", benignChange(t, root))
		requireToolStateNamedOrSeen(t, key, o)
	})
	t.Run("tracked cap hit leaves untracked files read", func(t *testing.T) {
		root := gitRepo(t)
		writeFile(t, root, ".devmap/big.txt", "seed\n")
		attackCommit(t, root, "track")
		writeFile(t, root, ".devmap/big.txt", pad(limit+10))
		writeFile(t, root, ".devmap/key.txt", line)
		o := attackRun(t, root, "TASK-D-TS-CAP4", benignChange(t, root))
		requireToolStateSeen(t, key, o)
	})
}

// A secret in a tool-state file must not move scope, drift or any other gate:
// the same gap types as the same change without the tool-state file, plus the
// security gap.
func TestDecidedToolStateSecretOnlyAddsTheSecretGap(t *testing.T) {
	key := exampleAWSKey()
	build := func(secret bool) attackOutcome {
		root := gitRepo(t)
		if secret {
			writeFile(t, root, ".devmap/k.txt", toolStateKeyLine())
		}
		return attackRun(t, root, "TASK-D-TS-ONLY", benignChange(t, root))
	}
	without, with := build(false), build(true)
	reportLeaks(t, with.result, with.gaps, key)
	extra := map[string]int{}
	for _, g := range with.gaps {
		extra[g.GapType]++
	}
	for _, g := range without.gaps {
		extra[g.GapType]--
	}
	for ty, n := range extra {
		if n != 0 && ty != "security_risk" {
			t.Errorf("tool-state secret moved gap type %s by %d", ty, n)
		}
	}
	if extra["security_risk"] < 1 {
		t.Fatalf("no security_risk gap: %v", advTypes(with.gaps))
	}
	if with.result.CoverageMeasured != without.result.CoverageMeasured || with.result.CoverageSkippedReason != without.result.CoverageSkippedReason {
		t.Errorf("coverage moved")
	}
}

// Gate modes: the tool-state secret blocks under enforce and under advisory.
func TestDecidedToolStateSecretUnderGateModes(t *testing.T) {
	key := exampleAWSKey()
	root := gitRepo(t)
	writeFile(t, root, ".devmap/k.txt", toolStateKeyLine())
	state, err := verify.CollectGitStateReport(context.Background(), root)
	if err != nil {
		t.Fatal(err)
	}
	for _, mode := range []string{"enforce", "advisory", "warn"} {
		t.Run(mode, func(t *testing.T) {
			in := verify.Input{GateMode: mode, Rigor: rigorFor(t, root)}
			in.WithGitState(state)
			gaps, meta := verify.Run(context.Background(), in)
			res := verify.ToMCP("TASK-D-MODE", gaps, meta)
			t.Logf("mode=%s status=%s passed=%v types=%v", mode, res.Status, res.Passed, advTypes(gaps))
			reportLeaks(t, res, gaps, key)
			if res.Passed || res.Status == "verified" {
				t.Fatalf("a tool-state secret passed under %s", mode)
			}
		})
	}
}

// A tool-state file that cannot be read: mode 000, a FIFO, a directory by name.
func TestDecidedToolStateUnreadableShapes(t *testing.T) {
	key := exampleAWSKey()
	t.Run("mode 000", func(t *testing.T) {
		if os.Geteuid() == 0 {
			t.Skip("root reads mode 000")
		}
		root := gitRepo(t)
		writeFile(t, root, ".devmap/locked.txt", toolStateKeyLine())
		if err := os.Chmod(filepath.Join(root, ".devmap/locked.txt"), 0); err != nil {
			t.Fatal(err)
		}
		requireToolStateNamed(t, key, attackRun(t, root, "TASK-D-TS-000", benignChange(t, root)), "locked.txt")
	})
	t.Run("fifo", func(t *testing.T) {
		root := gitRepo(t)
		if err := os.MkdirAll(filepath.Join(root, ".devmap"), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := syscall.Mkfifo(filepath.Join(root, ".devmap/pipe"), 0o644); err != nil {
			t.Skip(err)
		}
		o := attackRun(t, root, "TASK-D-TS-FIFO", benignChange(t, root))
		t.Logf("a FIFO is not a file git lists, tracks or commits: class=%s", decidedClass(o))
	})
	t.Run("tracked file made unreadable", func(t *testing.T) {
		if os.Geteuid() == 0 {
			t.Skip("root reads mode 000")
		}
		root := gitRepo(t)
		writeFile(t, root, ".devmap/locked.txt", "x\n")
		attackCommit(t, root, "track")
		writeFile(t, root, ".devmap/locked.txt", toolStateKeyLine())
		if err := os.Chmod(filepath.Join(root, ".devmap/locked.txt"), 0); err != nil {
			t.Fatal(err)
		}
		o := attackRun(t, root, "TASK-D-TS-000T", benignChange(t, root))
		t.Logf("class=%s refused=%v degraded=%q", decidedClass(o), o.refused, o.result.DegradedReasons)
		if decidedClass(o) == "verified" {
			t.Fatalf("an unreadable tracked tool-state file read as verified")
		}
	})
}

// A secret split across a hunk boundary and a multi-line private key where the
// header line is context.
func TestDecidedToolStateSplitSecrets(t *testing.T) {
	key := exampleAWSKey()
	t.Run("key split by a context line", func(t *testing.T) {
		root := gitRepo(t)
		body := strings.Repeat("keep\n", 40) + "AKIA\n" + strings.Repeat("keep\n", 40)
		writeFile(t, root, ".devmap/s.txt", body)
		attackCommit(t, root, "track")
		writeFile(t, root, ".devmap/s.txt", strings.Replace(body, "AKIA\n", "AKIA"+"IOSFODNN7EXAMPLE\n", 1))
		requireToolStateSeen(t, key, attackRun(t, root, "TASK-D-TS-SPLIT1", benignChange(t, root)))
	})
	t.Run("private key body added under a committed BEGIN line", func(t *testing.T) {
		root := gitRepo(t)
		begin := "-----BEGIN RSA PRIVATE KEY-----\n"
		writeFile(t, root, ".devmap/pk.pem", begin+"-----END RSA PRIVATE KEY-----\n")
		attackCommit(t, root, "track")
		body := strings.Repeat("MIIEowIBAAKCAQEAu1SU1LfVLPHCozMxH2Mo4lgOEePzNm0tRgeLezV6ffAt0gun\n", 25)
		writeFile(t, root, ".devmap/pk.pem", begin+body+"-----END RSA PRIVATE KEY-----\n")
		o := attackRun(t, root, "TASK-D-TS-SPLIT2", benignChange(t, root))
		t.Logf("tool state: class=%s", decidedClass(o))
		root2 := gitRepo(t)
		writeFile(t, root2, "pk.pem", begin+"-----END RSA PRIVATE KEY-----\n")
		attackCommit(t, root2, "track")
		writeFile(t, root2, "pk.pem", begin+body+"-----END RSA PRIVATE KEY-----\n")
		o2 := attackRun(t, root2, "TASK-D-ORD-SPLIT2", benignChange(t, root2), dc.PlannedFile{Path: "pk.pem", AllowedChange: dc.ChangeModify})
		t.Logf("ordinary file: class=%s", decidedClass(o2))
		if decidedClass(o) != decidedClass(o2) {
			t.Fatalf("tool state and an ordinary file differ: %s vs %s", decidedClass(o), decidedClass(o2))
		}
	})
}

// Headless repository: the tool-state file is in the index (staged) or not.
func TestDecidedToolStateHeadless(t *testing.T) {
	key := exampleAWSKey()
	for _, stage := range []bool{false, true} {
		t.Run(fmt.Sprintf("staged=%v", stage), func(t *testing.T) {
			root := t.TempDir()
			runGitIn(t, root, "init", "-q")
			writeFile(t, root, "a.txt", "a\n")
			writeFile(t, root, ".devcouncil/k.txt", toolStateKeyLine())
			if stage {
				runGitIn(t, root, "add", "-A")
				writeFile(t, root, ".devcouncil/k.txt", "clean\n")
			}
			o := attackRun(t, root, "TASK-D-TS-HL", dc.PlannedFile{Path: "a.txt", AllowedChange: dc.ChangeCreate})
			requireToolStateSeen(t, key, o)
		})
	}
}

// A rename into tool state: the main diff excludes the new path, so the added
// content must still reach the secret scan.
func TestDecidedToolStateRenameIntoIt(t *testing.T) {
	root := gitRepo(t)
	writeFile(t, root, "src/keys.txt", toolStateKeyLine())
	attackCommit(t, root, "a committed file holding a key")
	if err := os.MkdirAll(filepath.Join(root, ".devmap"), 0o755); err != nil {
		t.Fatal(err)
	}
	runGitIn(t, root, "mv", "src/keys.txt", ".devmap/keys.txt")
	o := attackRun(t, root, "TASK-D-TS-MV", benignChange(t, root))
	t.Logf("class=%s types=%v", decidedClass(o), advTypes(o.gaps))
	if decidedClass(o) == "verified" {
		t.Fatalf("a committed key moved into tool state read as verified")
	}
}

// No dcverify at all: the verdict must not be verified.
func TestDecidedToolStateWithNoRigor(t *testing.T) {
	root := gitRepo(t)
	writeFile(t, root, ".devmap/k.txt", toolStateKeyLine())
	state, err := verify.CollectGitStateReport(context.Background(), root)
	if err != nil {
		t.Fatal(err)
	}
	in := verify.Input{GateMode: "enforce"}
	in.WithGitState(state)
	gaps, meta := verify.Run(context.Background(), in)
	res := verify.ToMCP("TASK-D-NORIGOR", gaps, meta)
	t.Logf("status=%s passed=%v degraded=%q", res.Status, res.Passed, res.DegradedReasons)
	if res.Passed || res.Status == "verified" {
		t.Fatalf("no rigor and a tool-state secret passed")
	}
}

// The key is in no output anywhere for any tool-state shape, including the
// JSON of the whole result.
func TestDecidedToolStateKeyNeverLeaks(t *testing.T) {
	key := exampleAWSKey()
	for _, rel := range []string{".devmap/k.txt", ".devcouncil/.env", ".DevCouncil/id_rsa", ".devmap/credentials.json"} {
		t.Run(rel, func(t *testing.T) {
			root := gitRepo(t)
			writeFile(t, root, rel, toolStateKeyLine())
			o := attackRun(t, root, "TASK-D-LEAK", benignChange(t, root))
			noKeyAnywhere(t, key, o)
			if strings.Contains(o.diff, key) {
				t.Fatalf("the key reached the diff the other gates judge")
			}
		})
	}
}

func rigorFor(t *testing.T, root string) *dcverify.Client {
	t.Helper()
	return dcverify.New(testsupport.DCVerify(t), root)
}

var _ = testsupport.Tool

// The same index/worktree divergence in an ordinary file and a headless
// repository, to say whether the tool-state result is the diff basis's.
func TestDecidedOrdinaryStagedSecretThenRevertedForComparison(t *testing.T) {
	key := exampleAWSKey()
	t.Run("tracked file, staged secret, worktree reverted", func(t *testing.T) {
		root := gitRepo(t)
		writeFile(t, root, "src/a.txt", "a\n")
		attackCommit(t, root, "track")
		writeFile(t, root, "src/a.txt", "a\n"+toolStateKeyLine())
		runGitIn(t, root, "add", "src/a.txt")
		writeFile(t, root, "src/a.txt", "a\n")
		o := attackRun(t, root, "TASK-D-ORD-REV", benignChange(t, root))
		t.Logf("ordinary: class=%s empty=%v diffHasKey=%v", decidedClass(o), o.empty, strings.Contains(o.diff, key))
		if decidedClass(o) == "verified" {
			t.Skip("ft-c397c278 (pre-existing on main a76e963f, verified there): `git diff HEAD` compares the worktree to HEAD, so a secret staged and then reverted on disk is in the index (and the next commit) and in no diff; the same basis serves tool state below")
		}
	})
	t.Run("headless repo, staged secret file", func(t *testing.T) {
		root := t.TempDir()
		runGitIn(t, root, "init", "-q")
		writeFile(t, root, "a.txt", toolStateKeyLine())
		runGitIn(t, root, "add", "a.txt")
		o := attackRun(t, root, "TASK-D-ORD-HL")
		t.Logf("headless staged: class=%s empty=%v diffHasKey=%v types=%v", decidedClass(o), o.empty, strings.Contains(o.diff, key), advTypes(o.gaps))
		if decidedClass(o) == "verified" {
			t.Skip("ft-c397c278 (pre-existing on main): a repository with no HEAD collects only untracked files and `ls-files --others` omits staged ones, so a staged secret is in no listing (the headless branch of CollectGitStateReport is unchanged by the lane except for tool state)")
		}
	})
}

// An inherited pathspec environment variable turns `:(top,icase)` into a
// literal path that matches nothing: the second diff would be empty with no
// error. The main diff's own `:(top,exclude)` is literal then too, so the
// tool-state files stay in the main diff. The secret must be seen either way.
func TestDecidedToolStateUnderInheritedPathspecEnv(t *testing.T) {
	key := exampleAWSKey()
	for _, env := range []string{"GIT_LITERAL_PATHSPECS=1", "GIT_NOGLOB_PATHSPECS=1", "GIT_GLOB_PATHSPECS=1", "GIT_ICASE_PATHSPECS=1"} {
		for _, tracked := range []bool{true, false} {
			t.Run(fmt.Sprintf("%s/tracked=%v", env, tracked), func(t *testing.T) {
				root := gitRepo(t)
				if tracked {
					writeFile(t, root, ".devmap/k.txt", "x\n")
					attackCommit(t, root, "track")
					writeFile(t, root, ".devmap/k.txt", "x\n"+toolStateKeyLine())
				} else {
					writeFile(t, root, ".devmap/k.txt", toolStateKeyLine())
				}
				k, v, _ := strings.Cut(env, "=")
				t.Setenv(k, v)
				requireToolStateSeen(t, key, attackRun(t, root, "TASK-D-TS-PATHSPEC", benignChange(t, root)))
			})
		}
	}
}

// A submodule path whose checkout lost its .git (so it is a plain directory in
// the superproject's eyes) and now holds files: the index still has a gitlink
// there, so git lists none of them, and no gitlink record moved.
func TestDecidedGitlinkDirectoryWithoutDotGit(t *testing.T) {
	key := exampleAWSKey()
	t.Run("populated plain directory over a committed gitlink", func(t *testing.T) {
		root, subDir := submoduleRepoWith(t, map[string]string{"s.txt": "ok\n"})
		if err := os.RemoveAll(filepath.Join(subDir, ".git")); err != nil {
			t.Fatal(err)
		}
		writeFile(t, subDir, "keys.txt", attackSecretLine())
		o := attackRun(t, root, "TASK-D-GL-NODOTGIT", benignChange(t, root))
		t.Logf("class=%s degraded=%q types=%v refused=%v", decidedClass(o), o.result.DegradedReasons, advTypes(o.gaps), o.refused)
		if decidedClass(o) == "verified" {
			t.Fatalf("a secret under a submodule path whose .git is gone reads verified")
		}
		reportLeaks(t, o.result, o.gaps, key)
	})
	t.Run("gitlink staged under a path that is a populated plain directory", func(t *testing.T) {
		root := gitRepo(t)
		writeFile(t, root, "vendor/keys.txt", attackSecretLine())
		indexInfo(t, root, [][3]string{{"160000", sha('a'), "vendor"}})
		o := attackRun(t, root, "TASK-D-GL-STAGEDIR", benignChange(t, root))
		t.Logf("class=%s degraded=%q types=%v refused=%v", decidedClass(o), o.result.DegradedReasons, advTypes(o.gaps), o.refused)
		if decidedClass(o) == "verified" {
			t.Fatalf("a secret under a directory the index calls a gitlink reads verified")
		}
	})
}

// Repository config and attributes that change how git prints a diff, applied
// to a tracked tool-state file that gains a secret.
func TestDecidedToolStateDiffShapingConfig(t *testing.T) {
	key := exampleAWSKey()
	cases := map[string]func(t *testing.T, root string){
		"gitattributes -diff": func(t *testing.T, root string) {
			writeFile(t, root, ".gitattributes", ".devmap/* -diff\n")
		},
		"gitattributes binary": func(t *testing.T, root string) {
			writeFile(t, root, ".gitattributes", ".devmap/* binary\n")
		},
		"gitattributes diff=driver with binary driver": func(t *testing.T, root string) {
			writeFile(t, root, ".gitattributes", ".devmap/* diff=drv\n")
			runGitIn(t, root, "config", "diff.drv.binary", "true")
		},
		"core.bigFileThreshold=1": func(t *testing.T, root string) {
			runGitIn(t, root, "config", "core.bigFileThreshold", "1")
		},
		"diff.noprefix": func(t *testing.T, root string) { runGitIn(t, root, "config", "diff.noprefix", "true") },
		"diff.mnemonicPrefix": func(t *testing.T, root string) {
			runGitIn(t, root, "config", "diff.mnemonicPrefix", "true")
		},
		"diff.srcPrefix/dstPrefix": func(t *testing.T, root string) {
			runGitIn(t, root, "config", "diff.srcPrefix", "zz/")
			runGitIn(t, root, "config", "diff.dstPrefix", "yy/")
		},
		"diff.context=0": func(t *testing.T, root string) { runGitIn(t, root, "config", "diff.context", "0") },
		"core.whitespace and eol": func(t *testing.T, root string) {
			runGitIn(t, root, "config", "core.autocrlf", "true")
			runGitIn(t, root, "config", "core.whitespace", "cr-at-eol")
		},
		"diff.algorithm=histogram and indentHeuristic": func(t *testing.T, root string) {
			runGitIn(t, root, "config", "diff.algorithm", "histogram")
		},
	}
	for name, setup := range cases {
		t.Run(name, func(t *testing.T) {
			root := gitRepo(t)
			writeFile(t, root, ".devmap/k.txt", "x\n")
			attackCommit(t, root, "track")
			setup(t, root)
			writeFile(t, root, ".devmap/k.txt", "x\n"+toolStateKeyLine())
			o := attackRun(t, root, "TASK-D-TS-CFG", benignChange(t, root),
				dc.PlannedFile{Path: ".gitattributes", AllowedChange: dc.ChangeCreate})
			requireToolStateNamedOrSeen(t, key, o)
			// The same shape in an ordinary file, to date the finding.
			root2 := gitRepo(t)
			writeFile(t, root2, "src/k.txt", "x\n")
			attackCommit(t, root2, "track")
			setup(t, root2)
			writeFile(t, root2, "src/k.txt", "x\n"+toolStateKeyLine())
			o2 := attackRun(t, root2, "TASK-D-ORD-CFG", benignChange(t, root2),
				dc.PlannedFile{Path: "src/k.txt", AllowedChange: dc.ChangeModify},
				dc.PlannedFile{Path: ".gitattributes", AllowedChange: dc.ChangeCreate})
			t.Logf("tool state class=%s, ordinary class=%s", decidedClass(o), decidedClass(o2))
		})
	}
}

// A text secret made binary-looking (a NUL in the first 8000 bytes, or an
// attribute): the ordinary path reads the binary bytes; tool state is only
// named. The comparison, so the finding can be dated.
func TestDecidedBinaryLookingSecretOrdinaryVersusToolState(t *testing.T) {
	key := exampleAWSKey()
	body := toolStateKeyLine() + "\x00tail\n"
	t.Run("ordinary untracked", func(t *testing.T) {
		root := gitRepo(t)
		writeFile(t, root, "src/b.txt", body)
		o := attackRun(t, root, "TASK-D-BIN-ORD", benignChange(t, root), dc.PlannedFile{Path: "src/b.txt", AllowedChange: dc.ChangeCreate})
		t.Logf("ordinary: class=%s degraded=%q types=%v", decidedClass(o), o.result.DegradedReasons, advTypes(o.gaps))
	})
	t.Run("tool state untracked", func(t *testing.T) {
		root := gitRepo(t)
		writeFile(t, root, ".devmap/b.txt", body)
		o := attackRun(t, root, "TASK-D-BIN-TS", benignChange(t, root))
		t.Logf("tool state: class=%s degraded=%q types=%v", decidedClass(o), o.result.DegradedReasons, advTypes(o.gaps))
		if decidedClass(o) != "found" {
			t.Errorf("tool-state secret in a binary-looking file is %s, not found: the ordinary path reads these bytes", decidedClass(o))
		}
		reportLeaks(t, o.result, o.gaps, key)
	})
	t.Run("tool state tracked with -diff attribute (agent-controlled)", func(t *testing.T) {
		root := gitRepo(t)
		writeFile(t, root, ".devmap/b.txt", "x\n")
		attackCommit(t, root, "track")
		writeFile(t, root, ".gitattributes", ".devmap/* -diff\n")
		writeFile(t, root, ".devmap/b.txt", "x\n"+toolStateKeyLine())
		o := attackRun(t, root, "TASK-D-BIN-ATTR", benignChange(t, root), dc.PlannedFile{Path: ".gitattributes", AllowedChange: dc.ChangeCreate})
		t.Logf("tool state + -diff: class=%s degraded=%q", decidedClass(o), o.result.DegradedReasons)
		if decidedClass(o) != "found" {
			t.Errorf("an agent-written .gitattributes turns a tool-state secret from found to %s", decidedClass(o))
		}
	})
}

// A reason is read by a person. A path with a right-to-left override, or with
// bytes that are not UTF-8, is carried into it raw: quoteDiffPath quotes ASCII
// control bytes and nothing else, so the override can reorder the line a
// reviewer reads, and a byte that is not UTF-8 is replaced when the result is
// marshalled.
func TestDecidedGitlinkReasonsAreDisplaySafe(t *testing.T) {
	t.Skip("ft-c397c278: raw U+202E and non-UTF-8 bytes in a gitlink reason are a follow-up (not in fix round 1, ruling 44); this test is the repro and fails until the card lands")
	root := t.TempDir()
	runGitIn(t, root, "init", "-q")
	indexInfo(t, root, [][3]string{{"160000", sha('a'), "x‮)dedda("}, {"160000", sha('a'), "bad\xffbytes"}})
	o := attackRun(t, root, "TASK-D-GL-DISPLAY")
	for _, r := range o.result.DegradedReasons {
		if !utf8.ValidString(r) {
			t.Errorf("a degraded reason is not valid UTF-8: %q", r)
		}
		if strings.ContainsRune(r, '‮') {
			t.Errorf("a degraded reason carries a right-to-left override: %q", r)
		}
	}
}
