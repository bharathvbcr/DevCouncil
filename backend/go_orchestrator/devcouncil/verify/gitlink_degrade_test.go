package verify_test

import (
	"bytes"
	"compress/zlib"
	"crypto/sha1" // #nosec G505 -- the commit-graph trailer is a SHA-1 of the file
	"encoding/binary"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// Every submodule gitlink a change moves is named as not scanned.
//
// Owner decision (2026-10-28): the commits a moved gitlink moved across are not
// read. `git diff HEAD` shows the move as one `Subproject commit` line, and a
// credential added in one submodule commit and removed in the next is in the
// history and in neither end, so the run says so: it degrades, in
// degraded_reasons, with one reason per gitlink. These tests hold the host to
// that end to end, through the real dcverify. They replace the tests of the
// range reader (a277ec2f, 6c270782, 65b6ee20) that parked with it: each of
// those asserted a credential in a range was found or the range was named, and
// the range is now always named.

const gitlinkReason = "submodule commit range not scanned: "

// requireDegraded fails unless the run was not refused, did not pass, and says
// in its degraded reasons that part of the change was not scanned, naming each
// of substrings. The credential is in no reason.
func requireDegraded(t *testing.T, key string, o attackOutcome, substrings ...string) {
	t.Helper()
	if o.refused != nil {
		t.Fatalf("the run was refused rather than degraded: %v", o.refused)
	}
	reportLeaks(t, o.result, o.gaps, key)
	for _, reason := range o.result.DegradedReasons {
		if strings.Contains(reason, key) {
			t.Fatal("a degraded reason contains the credential")
		}
	}
	if o.result.Passed || o.result.Status == "verified" {
		t.Fatalf("status=%s passed=%v with part of the change unscanned", o.result.Status, o.result.Passed)
	}
	if o.result.Status != "degraded" {
		var blocking []string
		for _, g := range o.gaps {
			if g.Blocking {
				blocking = append(blocking, g.GapType+": "+g.Description)
			}
		}
		t.Fatalf("status=%s, want degraded: blocking=%q degraded=%q", o.result.Status, blocking, o.result.DegradedReasons)
	}
	joined := strings.Join(o.result.DegradedReasons, "\n")
	for _, want := range substrings {
		if !strings.Contains(joined, want) {
			t.Fatalf("no degraded reason names %q: %q", want, o.result.DegradedReasons)
		}
	}
}

// requireGitlink is requireDegraded for one gitlink reason.
func requireGitlink(t *testing.T, key string, o attackOutcome, path, shape string) {
	t.Helper()
	requireDegraded(t, key, o, gitlinkReason+path+" ("+shape+")")
}

// requireNotPassedAndNamed is requireGitlink for a change that dcverify already
// refuses outright: git writes a typechange as two sections for one path, and
// the parser rejects a diff naming a path twice. Such a run is blocked by a
// rigor_check_unavailable gap, which is stronger than degraded. It must never
// pass, and if nothing blocks it, it must be degraded with the reason.
func requireNotPassedAndNamed(t *testing.T, key string, o attackOutcome, path, shape string) {
	t.Helper()
	if o.refused != nil {
		t.Fatalf("the run was refused rather than degraded: %v", o.refused)
	}
	reportLeaks(t, o.result, o.gaps, key)
	if o.result.Passed || o.result.Status == "verified" {
		t.Fatalf("status=%s passed=%v with part of the change unscanned", o.result.Status, o.result.Passed)
	}
	if o.result.Status == "blocked" {
		for _, g := range o.gaps {
			if g.Blocking && (g.GapType == "rigor_check_unavailable" || g.GapType == "task_not_implemented") {
				return
			}
		}
		t.Fatalf("blocked, but not by a refusal to read the change: %v", advTypes(o.gaps))
	}
	requireGitlink(t, key, o, path, shape)
}

// subCommitFile writes a file in the submodule checkout and commits it.
func subCommitFile(t *testing.T, subDir, name, body, msg string) {
	t.Helper()
	writeFile(t, subDir, name, body)
	runGitIn(t, subDir, "add", "-A")
	runGitIn(t, subDir, "commit", "-qm", msg)
}

func gitOutIn(t *testing.T, dir string, args ...string) string {
	t.Helper()
	cmd := exec.Command(testsupport.Tool(t, "git"), args...)
	cmd.Dir = dir
	out, err := cmd.Output()
	if err != nil {
		t.Fatalf("git %v in %s: %v", args, dir, err)
	}
	return strings.TrimSpace(string(out))
}

// everything is every string an operator or model could be shown for a run,
// including the JSON of the whole result and the refusal.
func everything(t *testing.T, o attackOutcome) string {
	t.Helper()
	var b strings.Builder
	if o.refused != nil {
		b.WriteString(o.refused.Error())
	}
	for _, v := range []any{o.result, o.gaps} {
		raw, err := json.Marshal(v)
		if err != nil {
			t.Fatal(err)
		}
		b.Write(raw)
	}
	return b.String()
}

func noKeyAnywhere(t *testing.T, key string, o attackOutcome) {
	t.Helper()
	if strings.Contains(everything(t, o), key) {
		t.Fatalf("the credential is in the run's output: %s", everything(t, o))
	}
}

// freshSource makes a repository with one commit, for `git submodule add`.
func freshSource(t *testing.T, name, body string) string {
	t.Helper()
	src := filepath.Join(t.TempDir(), name)
	if err := os.MkdirAll(src, 0o755); err != nil {
		t.Fatal(err)
	}
	runGitIn(t, src, "init", "-q")
	writeFile(t, src, "f.txt", body)
	runGitIn(t, src, "add", "-A")
	runGitIn(t, src, "commit", "-qm", "s")
	return src
}

func addSubmodule(t *testing.T, root, src, path string) {
	t.Helper()
	runGitIn(t, root, "-c", "protocol.file.allow=always", "submodule", "add", "-q", src, path)
}

// ---------------------------------------------------------------- the shapes

// A credential added in one submodule commit and removed in the next is in the
// history and in neither end: the move is named, whatever the history holds.
func TestGitlinkMovedAcrossASecretIsDegraded(t *testing.T) {
	key := exampleAWSKey()
	root, subDir := submoduleRepoWith(t, map[string]string{"s.txt": "ok\n"})
	subCommitFile(t, subDir, "a.txt", "innocent\n", "A")
	subCommitFile(t, subDir, "keys.txt", attackSecretLine(), "B adds the secret")
	runGitIn(t, subDir, "rm", "-q", "keys.txt")
	runGitIn(t, subDir, "commit", "-qm", "C removes it")
	o := attackRun(t, root, "TASK-GL-RANGE", dc.PlannedFile{Path: "sub", AllowedChange: dc.ChangeModify})
	requireGitlink(t, key, o, "sub", "moved")
	noKeyAnywhere(t, key, o)
}

// The shapes a moved gitlink can take in the submodule's history are all one
// outcome: none of them reads as clean, none is an error.
func TestGitlinkMovedRangesOfEveryShapeAreDegraded(t *testing.T) {
	key := exampleAWSKey()
	builders := map[string]func(t *testing.T, subDir string){
		"binary change": func(t *testing.T, subDir string) {
			subCommitFile(t, subDir, "blob.bin", "\x00"+attackSecretLine(), "binary")
		},
		"a picture and a text secret": func(t *testing.T, subDir string) {
			subCommitFile(t, subDir, "keys.txt", attackSecretLine(), "text secret")
			subCommitFile(t, subDir, "logo.png", "\x89PNG\x00\x00binary", "a picture")
		},
		"a merge holding the only copy": func(t *testing.T, subDir string) {
			trunk := gitOutIn(t, subDir, "rev-parse", "--abbrev-ref", "HEAD")
			runGitIn(t, subDir, "checkout", "-q", "-b", "side")
			subCommitFile(t, subDir, "side.txt", "on the side\n", "side")
			runGitIn(t, subDir, "checkout", "-q", trunk)
			subCommitFile(t, subDir, "again.txt", "first\n", "first version")
			runGitIn(t, subDir, "merge", "-q", "--no-ff", "--no-commit", "side")
			writeFile(t, subDir, "merge-only.txt", attackSecretLine())
			runGitIn(t, subDir, "add", "merge-only.txt")
			runGitIn(t, subDir, "commit", "-qm", "merge")
		},
		"an empty commit": func(t *testing.T, subDir string) {
			runGitIn(t, subDir, "commit", "-q", "--allow-empty", "-m", "nothing")
		},
		"moved backwards": func(t *testing.T, subDir string) {
			// The superproject records the second commit, then the checkout
			// goes back to the first.
			subCommitFile(t, subDir, "later.txt", "later\n", "later")
			runGitIn(t, filepath.Dir(subDir), "add", "sub")
			runGitIn(t, filepath.Dir(subDir), "commit", "-qm", "record the later commit")
			runGitIn(t, subDir, "checkout", "-q", "HEAD~1")
		},
		"onto unrelated history": func(t *testing.T, subDir string) {
			runGitIn(t, subDir, "checkout", "-q", "--orphan", "elsewhere")
			subCommitFile(t, subDir, "other.txt", attackSecretLine(), "unrelated root")
		},
		"files with awkward names": func(t *testing.T, subDir string) {
			for _, name := range []string{`back\slash.txt`, "tab\tname.txt", "sp ace/in dir.txt", "éé.txt"} {
				subCommitFile(t, subDir, name, attackSecretLine(), "adds "+strconv.Quote(name))
				runGitIn(t, subDir, "rm", "-q", "--", name)
				runGitIn(t, subDir, "commit", "-qm", "removes it")
			}
		},
		"a nested submodule moved under ignore = all": func(t *testing.T, subDir string) {
			inner := freshSource(t, "inner-src", "i\n")
			runGitIn(t, subDir, "-c", "protocol.file.allow=always", "submodule", "add", "-q", inner, "inner")
			runGitIn(t, subDir, "config", "-f", ".gitmodules", "submodule.inner.ignore", "all")
			runGitIn(t, subDir, "add", ".gitmodules")
			runGitIn(t, subDir, "commit", "-qm", "add inner, ignored")
			subCommitFile(t, filepath.Join(subDir, "inner"), "j.txt", attackSecretLine(), "inner moves, carrying a secret")
			runGitIn(t, subDir, "update-index", "--cacheinfo", "160000,"+gitOutIn(t, filepath.Join(subDir, "inner"), "rev-parse", "HEAD")+",inner")
			runGitIn(t, subDir, "commit", "-qm", "bump inner")
		},
	}
	for name, build := range builders {
		t.Run(name, func(t *testing.T) {
			root, subDir := submoduleRepoWith(t, map[string]string{"s.txt": "ok\n"})
			build(t, subDir)
			o := attackRun(t, root, "TASK-GL-SHAPES", dc.PlannedFile{Path: "sub", AllowedChange: dc.ChangeModify})
			requireGitlink(t, key, o, "sub", "moved")
			noKeyAnywhere(t, key, o)
		})
	}
}

// A bump whose range holds nothing but placeholders an ordinary change may
// carry used to verify. It is now degraded: the run did not read the range, so
// it cannot say the range is fine.
func TestAttackLegitSubmoduleBumpVerifies(t *testing.T) {
	key := exampleAWSKey()
	root, subDir := submoduleRepoWith(t, map[string]string{"s.txt": "ok\n"})
	subCommitFile(t, subDir, "src/lib.rs", "pub fn later() -> u32 {\n    todo!()\n}\n", "a placeholder")
	subCommitFile(t, subDir, "impl.py", "def later():\n    raise NotImplementedError\n", "another")
	o := attackRun(t, root, "TASK-ATK-SUB-LEGIT", dc.PlannedFile{Path: "sub", AllowedChange: dc.ChangeModify})
	requireGitlink(t, key, o, "sub", "moved")
	for _, g := range o.gaps {
		if g.GapType == "stub_detected" || g.GapType == "stub_declared" {
			t.Fatalf("a stub gap came from the submodule's range: %s %s", g.GapType, g.Description)
		}
	}
}

func TestGitlinkAddedIsDegraded(t *testing.T) {
	key := exampleAWSKey()
	root, _ := submoduleRepoWith(t, map[string]string{"s.txt": "ok\n"})
	addSubmodule(t, root, freshSource(t, "fresh-src", attackSecretLine()), "fresh")
	o := attackRun(t, root, "TASK-GL-ADDED",
		dc.PlannedFile{Path: "fresh", AllowedChange: dc.ChangeCreate},
		dc.PlannedFile{Path: ".gitmodules", AllowedChange: dc.ChangeModify})
	requireGitlink(t, key, o, "fresh", "added")
	noKeyAnywhere(t, key, o)
}

func TestGitlinkDeletedIsDegraded(t *testing.T) {
	key := exampleAWSKey()
	root, _ := submoduleRepoWith(t, map[string]string{"s.txt": "ok\n"})
	runGitIn(t, root, "rm", "-q", "-f", "sub")
	o := attackRun(t, root, "TASK-GL-DELETED",
		dc.PlannedFile{Path: "sub", AllowedChange: dc.ChangeDelete},
		dc.PlannedFile{Path: ".gitmodules", AllowedChange: dc.ChangeModify})
	requireGitlink(t, key, o, "sub", "deleted")
}

func TestGitlinkTypechangeInBothDirectionsIsDegraded(t *testing.T) {
	key := exampleAWSKey()
	t.Run("a file replaced by a submodule", func(t *testing.T) {
		root := gitRepo(t)
		writeFile(t, root, "ts", "a plain file\n")
		attackCommit(t, root, "add ts")
		runGitIn(t, root, "rm", "-q", "-f", "ts")
		addSubmodule(t, root, freshSource(t, "ts-src", "x\n"), "ts")
		o := attackRun(t, root, "TASK-GL-TC-IN",
			dc.PlannedFile{Path: "ts", AllowedChange: dc.ChangeModify},
			dc.PlannedFile{Path: ".gitmodules", AllowedChange: dc.ChangeModify})
		requireNotPassedAndNamed(t, key, o, "ts", "typechange: file to submodule")
	})
	t.Run("a submodule replaced by a file", func(t *testing.T) {
		root, subDir := submoduleRepoWith(t, map[string]string{"s.txt": "ok\n"})
		runGitIn(t, root, "rm", "-q", "--cached", "sub")
		if err := os.RemoveAll(subDir); err != nil {
			t.Fatal(err)
		}
		writeFile(t, root, "sub", "now a plain file\n")
		runGitIn(t, root, "add", "sub")
		o := attackRun(t, root, "TASK-GL-TC-OUT",
			dc.PlannedFile{Path: "sub", AllowedChange: dc.ChangeModify},
			dc.PlannedFile{Path: ".gitmodules", AllowedChange: dc.ChangeModify})
		requireNotPassedAndNamed(t, key, o, "sub", "typechange: submodule to file")
	})
}

// A gitlink the index records that the superproject's own history never saw
// has no commit in the submodule to read. It is moved all the same.
func TestGitlinkRecordingACommitTheSubmoduleLacksIsDegraded(t *testing.T) {
	key := exampleAWSKey()
	root, subDir := submoduleRepoWith(t, map[string]string{"s.txt": "ok\n"})
	fake := strings.Repeat("ab", 20)
	runGitIn(t, root, "update-index", "--cacheinfo", "160000,"+fake+",sub")
	runGitIn(t, root, "commit", "-qm", "record a commit the submodule does not have")
	subCommitFile(t, subDir, "later.txt", attackSecretLine(), "later")
	o := attackRun(t, root, "TASK-GL-OLD", dc.PlannedFile{Path: "sub", AllowedChange: dc.ChangeModify})
	requireGitlink(t, key, o, "sub", "moved")
	if strings.Contains(strings.Join(o.result.DegradedReasons, "\n"), fake[:12]) {
		t.Fatalf("a reason carries a commit id: %q", o.result.DegradedReasons)
	}
}

// A staged bump that the submodule's checkout has not followed is a change the
// next commit makes, and `git diff HEAD` compares the checkout, not the index.
func TestGitlinkStagedBumpTheCheckoutHasNotFollowedIsDegraded(t *testing.T) {
	key := exampleAWSKey()
	root, subDir := submoduleRepoWith(t, map[string]string{"s.txt": "ok\n"})
	subCommitFile(t, subDir, "later.txt", attackSecretLine(), "later")
	head := gitOutIn(t, subDir, "rev-parse", "HEAD")
	runGitIn(t, subDir, "reset", "-q", "--hard", "HEAD~1")
	runGitIn(t, root, "update-index", "--cacheinfo", "160000,"+head+",sub")
	o := attackRun(t, root, "TASK-GL-STAGED", dc.PlannedFile{Path: "sub", AllowedChange: dc.ChangeModify})
	requireNotPassedAndNamed(t, key, o, "sub", "moved")
}

// A submodule the checkout does not hold (a clone that never ran
// `submodule update`) whose recorded commit changed is still a move.
func TestGitlinkNotCheckedOutIsDegraded(t *testing.T) {
	key := exampleAWSKey()
	root, _ := submoduleRepoWith(t, map[string]string{"s.txt": "ok\n"})
	clone := filepath.Join(t.TempDir(), "clone")
	runGitIn(t, filepath.Dir(clone), "clone", "-q", root, clone)
	fake := strings.Repeat("cd", 20)
	runGitIn(t, clone, "update-index", "--cacheinfo", "160000,"+fake+",sub")
	o := attackRun(t, clone, "TASK-GL-UNINIT", dc.PlannedFile{Path: "sub", AllowedChange: dc.ChangeModify})
	requireGitlink(t, key, o, "sub", "moved")
}

// A repository with no commit yet has no HEAD to compare against: every gitlink
// in it is new, and it is not a refusal.
func TestHeadlessRepoWithSubmoduleDoesNotError(t *testing.T) {
	key := exampleAWSKey()
	root := t.TempDir()
	runGitIn(t, root, "init", "-q")
	addSubmodule(t, root, freshSource(t, "head-src", "ok\n"), "sub")
	o := attackRun(t, root, "TASK-GL-HEADLESS")
	requireGitlink(t, key, o, "sub", "added")
}

// ---------------------------------------------------------------- hiding it

// `ignore = all`, in the superproject's .gitmodules or its config, makes plain
// `git diff` print nothing for a gitlink. The listing is asked on the command
// line, which overrides both.
func TestGitlinkIgnoreAllDoesNotHideARecord(t *testing.T) {
	key := exampleAWSKey()
	root, subDir := submoduleRepoWith(t, map[string]string{"s.txt": "ok\n"})
	runGitIn(t, root, "config", "-f", ".gitmodules", "submodule.sub.ignore", "all")
	runGitIn(t, root, "config", "diff.ignoreSubmodules", "all")
	runGitIn(t, root, "config", "submodule.sub.ignore", "all")
	runGitIn(t, root, "add", ".gitmodules")
	runGitIn(t, root, "commit", "-qm", "ignore all")
	subCommitFile(t, subDir, "keys.txt", attackSecretLine(), "adds the secret")
	runGitIn(t, subDir, "rm", "-q", "keys.txt")
	runGitIn(t, subDir, "commit", "-qm", "removes it")
	o := attackRun(t, root, "TASK-GL-IGNORE", dc.PlannedFile{Path: "sub", AllowedChange: dc.ChangeModify})
	requireGitlink(t, key, o, "sub", "moved")
	noKeyAnywhere(t, key, o)
}

// A submodule's own tracked .gitmodules says `ignore = all` for a nested one.
// The outer gitlink moved with it, and that is what is named.
func TestAttack2NestedGitlinkMoveHiddenByTrackedGitmodulesIsNotVerified(t *testing.T) {
	key := exampleAWSKey()
	root, subDir := submoduleRepoWith(t, map[string]string{"s.txt": "ok\n"})
	runGitIn(t, subDir, "-c", "protocol.file.allow=always", "submodule", "add", "-q", freshSource(t, "inner-src", "i\n"), "inner")
	runGitIn(t, subDir, "config", "-f", ".gitmodules", "submodule.inner.ignore", "all")
	runGitIn(t, subDir, "add", ".gitmodules")
	runGitIn(t, subDir, "commit", "-qm", "add inner, ignored")
	runGitIn(t, root, "add", "sub")
	runGitIn(t, root, "commit", "-qm", "record sub with inner")
	innerDir := filepath.Join(subDir, "inner")
	subCommitFile(t, innerDir, "j.txt", attackSecretLine(), "inner moves, carrying a secret")
	runGitIn(t, subDir, "update-index", "--cacheinfo", "160000,"+gitOutIn(t, innerDir, "rev-parse", "HEAD")+",inner")
	runGitIn(t, subDir, "commit", "-qm", "bump inner")
	o := attackRun(t, root, "TASK-ATK2-NESTED", dc.PlannedFile{Path: "sub", AllowedChange: dc.ChangeModify})
	requireGitlink(t, key, o, "sub", "moved")
}

// Whatever the submodule's own config names must not run while the change is
// read, and a hostile .gitmodules URL is never followed: nothing is read.
func TestGitlinkReadRunsNothingTheSubmoduleNames(t *testing.T) {
	key := exampleAWSKey()
	root, subDir := submoduleRepoWith(t, map[string]string{"s.txt": "ok\n"})
	marker := filepath.Join(t.TempDir(), "ran")
	script := filepath.Join(t.TempDir(), "hostile.sh")
	if err := os.WriteFile(script, []byte("#!/bin/sh\necho ran >> '"+marker+"'\ncat\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	subCommitFile(t, subDir, "keys.txt", attackSecretLine(), "adds the secret")
	writeFile(t, subDir, ".gitattributes", "keys.txt diff=keys filter=hostile\n")
	runGitIn(t, subDir, "add", ".gitattributes")
	runGitIn(t, subDir, "commit", "-qm", "attributes")
	// The config goes in last: git run by this test would run it too.
	for _, kv := range [][2]string{
		{"diff.external", script},
		{"diff.noprefix", "true"},
		{"diff.mnemonicPrefix", "true"},
		{"core.fsmonitor", script},
		{"core.hooksPath", filepath.Dir(script)},
		{"diff.keys.textconv", script},
		{"filter.hostile.clean", script},
	} {
		runGitIn(t, subDir, "config", kv[0], kv[1])
	}
	if _, err := os.Stat(marker); err == nil {
		t.Fatal("the test's own setup ran the hostile program")
	}
	writeFile(t, root, ".gitmodules", "[submodule \"sub\"]\n\tpath = sub\n\turl = ext::sh -c 'echo ran >> "+marker+"'\n")
	o := attackRun(t, root, "TASK-ATK-SUB-HOSTILE",
		dc.PlannedFile{Path: "sub", AllowedChange: dc.ChangeModify},
		dc.PlannedFile{Path: ".gitmodules", AllowedChange: dc.ChangeModify})
	if _, err := os.Stat(marker); err == nil {
		t.Fatal("a program the submodule's config or .gitmodules names ran while the change was read")
	}
	requireGitlink(t, key, o, "sub", "moved")
}

// Presentation config in the submodule changes how git prints its own history,
// and nothing about whether the gitlink moved.
func TestAttack2HostileSubmoduleConfigStillNamesTheMove(t *testing.T) {
	key := exampleAWSKey()
	for _, kv := range [][2]string{
		{"diff.suppressBlankEmpty", "true"},
		{"diff.context", "0"},
		{"core.abbrev", "4"},
		{"diff.interHunkContext", "9"},
		{"color.ui", "always"},
		{"core.quotePath", "true"},
		{"i18n.logOutputEncoding", "UTF-16"},
		{"format.pretty", "oneline"},
		{"log.decorate", "full"},
		{"diff.noprefix", "true"},
	} {
		t.Run(kv[0]+"="+kv[1], func(t *testing.T) {
			root, subDir := submoduleRepoWith(t, map[string]string{"s.txt": "ok\n"})
			subCommitFile(t, subDir, "keys.txt", "first\n\nthird\n"+attackSecretLine()+"\nfifth\n", "adds the secret")
			subCommitFile(t, subDir, "keys.txt", "first\n\nthird\nfifth\n", "removes it")
			runGitIn(t, subDir, "config", kv[0], kv[1])
			o := attackRun(t, root, "TASK-ATK2-CONFIG", dc.PlannedFile{Path: "sub", AllowedChange: dc.ChangeModify})
			requireGitlink(t, key, o, "sub", "moved")
		})
	}
}

// A replace ref in the submodule swaps the commit that adds a credential for a
// clean twin. Nothing is read, so nothing is swapped, and the move is named.
func TestAttack2ReplaceRefHidesNothingBecauseNothingIsRead(t *testing.T) {
	key := exampleAWSKey()
	root, subDir := submoduleRepoWith(t, map[string]string{"s.txt": "ok\n"})
	subCommitFile(t, subDir, "a.txt", "innocent\n", "A")
	subCommitFile(t, subDir, "keys.txt", attackSecretLine(), "B adds the secret")
	bad := gitOutIn(t, subDir, "rev-parse", "HEAD")
	runGitIn(t, subDir, "rm", "-q", "keys.txt")
	runGitIn(t, subDir, "commit", "-qm", "C removes it")
	tree := gitOutIn(t, subDir, "rev-parse", bad+"^^{tree}")
	parent := gitOutIn(t, subDir, "rev-parse", bad+"^")
	clean := gitOutIn(t, subDir, "commit-tree", tree, "-p", parent, "-m", "B clean")
	runGitIn(t, subDir, "replace", bad, clean)
	o := attackRun(t, root, "TASK-ATK2-REPLACE", dc.PlannedFile{Path: "sub", AllowedChange: dc.ChangeModify})
	requireGitlink(t, key, o, "sub", "moved")
}

// Two submodules, one of which records a commit it does not have: each moved
// gitlink has its own reason, and neither hides the other.
func TestAttack2UnscannableSubmoduleDoesNotHideTheOther(t *testing.T) {
	key := exampleAWSKey()
	root, one := submoduleRepoWith(t, map[string]string{"s.txt": "ok\n"})
	addSubmodule(t, root, freshSource(t, "two-src", "s\n"), "two")
	runGitIn(t, root, "commit", "-qm", "two")
	two := filepath.Join(root, "two")
	fake := strings.Repeat("ab", 20)
	runGitIn(t, root, "update-index", "--cacheinfo", "160000,"+fake+",sub")
	runGitIn(t, root, "commit", "-qm", "sub records a commit it does not have")
	subCommitFile(t, one, "x.txt", "x\n", "x")
	subCommitFile(t, two, "keys.txt", attackSecretLine(), "adds the secret")
	runGitIn(t, two, "rm", "-q", "keys.txt")
	runGitIn(t, two, "commit", "-qm", "removes it")
	o := attackRun(t, root, "TASK-ATK2-TWO",
		dc.PlannedFile{Path: "sub", AllowedChange: dc.ChangeModify},
		dc.PlannedFile{Path: "two", AllowedChange: dc.ChangeModify})
	requireDegraded(t, key, o, gitlinkReason+"sub (moved)", gitlinkReason+"two (moved)")
}

// Paths that differ only by a trailing space are two files: a move under one
// is not hidden by the other.
func TestAttack2TrailingSpaceTwinPathsInOneCommitAreDegraded(t *testing.T) {
	key := exampleAWSKey()
	root, subDir := submoduleRepoWith(t, map[string]string{"s.txt": "ok\n"})
	writeFile(t, subDir, "f.txt", "clean\n")
	writeFile(t, subDir, "f.txt ", attackSecretLine())
	runGitIn(t, subDir, "add", "-A")
	runGitIn(t, subDir, "commit", "-qm", "twins")
	runGitIn(t, subDir, "rm", "-q", "--", "f.txt ")
	runGitIn(t, subDir, "commit", "-qm", "remove the twin")
	o := attackRun(t, root, "TASK-ATK2-TWINS", dc.PlannedFile{Path: "sub", AllowedChange: dc.ChangeModify})
	requireGitlink(t, key, o, "sub", "moved")
}

// A submodule path that has been replaced by a symlink is not read through.
// The collection may refuse it (it does, because the dirtiness walk will not
// follow a symlink) or name it; it never passes.
func TestAttackSymlinkedSubmodulePathIsNotPassed(t *testing.T) {
	key := exampleAWSKey()
	root, subDir := submoduleRepoWith(t, map[string]string{"s.txt": "ok\n"})
	outside := t.TempDir()
	runGitIn(t, outside, "init", "-q")
	writeFile(t, outside, "keys.txt", attackSecretLine())
	runGitIn(t, outside, "add", "-A")
	runGitIn(t, outside, "commit", "-qm", "outside")
	if err := os.RemoveAll(subDir); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(outside, subDir); err != nil {
		t.Fatal(err)
	}
	o := attackRun(t, root, "TASK-ATK-SUB-SYMLINK", dc.PlannedFile{Path: "sub", AllowedChange: dc.ChangeModify})
	if o.refused != nil {
		if strings.Contains(o.refused.Error(), key) {
			t.Fatal("the refusal quotes the credential")
		}
		return
	}
	reportLeaks(t, o.result, o.gaps, key)
	if o.result.Passed || o.result.Status == "verified" {
		t.Fatalf("a submodule path replaced by a symlink verified: status=%s types=%v", o.result.Status, advTypes(o.gaps))
	}
}

// ---------------------------------------------------------------- where it sits

// A gitlink under a tool-state directory is not the agent's work to judge, but
// it is a gitlink the change moves, and it is named.
func TestAttack2SubmoduleAtToolStatePathIsNotSilentlyDropped(t *testing.T) {
	key := exampleAWSKey()
	root := gitRepo(t)
	addSubmodule(t, root, freshSource(t, "mod-src", "m\n"), ".devcouncil/mod")
	attackCommit(t, root, "mod under tool state")
	subCommitFile(t, filepath.Join(root, ".devcouncil", "mod"), "later.txt", attackSecretLine(), "later")
	o := attackRun(t, root, "TASK-ATK2-TOOLSTATE-GL", benignChange(t, root))
	requireDegraded(t, key, o, gitlinkReason+".devcouncil/mod (moved)")
}

// A gitlink whose path is spelled like a credential file prints no path.
func TestGitlinkSecretSpelledPathIsWithheldFromTheReason(t *testing.T) {
	key := exampleAWSKey()
	root := gitRepo(t)
	addSubmodule(t, root, freshSource(t, "cred-src", "c\n"), "credentials/vault")
	attackCommit(t, root, "vault")
	subCommitFile(t, filepath.Join(root, "credentials", "vault"), "later.txt", "later\n", "later")
	o := attackRun(t, root, "TASK-GL-SECRETPATH", benignChange(t, root),
		dc.PlannedFile{Path: "credentials/vault", AllowedChange: dc.ChangeModify})
	requireDegraded(t, key, o, gitlinkReason+"secret_path (moved)")
	for _, reason := range o.result.DegradedReasons {
		if strings.Contains(reason, "credentials") || strings.Contains(reason, "vault") {
			t.Fatalf("a reason prints a secret-spelled path: %q", reason)
		}
	}
}

var hexID = regexp.MustCompile(`[0-9a-f]{12,}`)

// A reason carries a path and a shape word, never a commit id.
func TestGitlinkReasonsCarryNoCommitIDs(t *testing.T) {
	key := exampleAWSKey()
	root, subDir := submoduleRepoWith(t, map[string]string{"s.txt": "ok\n"})
	subCommitFile(t, subDir, "a.txt", "a\n", "a")
	o := attackRun(t, root, "TASK-GL-IDS", dc.PlannedFile{Path: "sub", AllowedChange: dc.ChangeModify})
	requireGitlink(t, key, o, "sub", "moved")
	for _, reason := range o.result.DegradedReasons {
		if hexID.MatchString(reason) {
			t.Fatalf("a reason carries a commit id: %q", reason)
		}
	}
}

// ---------------------------------------------------------------- leaks

// Whatever the range holds, the credential is in no output: the range is not
// read, so there is nothing to echo, and a credential used as a path inside the
// range is not echoed either.
func TestAttack2KeyInSubmoduleRangeNeverAppearsInAnyOutput(t *testing.T) {
	key := exampleAWSKey()
	for name, build := range map[string]func(t *testing.T, subDir string){
		"content only": func(t *testing.T, subDir string) {
			subCommitFile(t, subDir, "k.txt", attackSecretLine(), "adds")
		},
		"key as file name and content": func(t *testing.T, subDir string) {
			subCommitFile(t, subDir, key+".txt", attackSecretLine(), "adds")
		},
		"key in a backslash name dcverify refuses": func(t *testing.T, subDir string) {
			subCommitFile(t, subDir, key+`\x.txt`, attackSecretLine(), "adds")
		},
		"key in a directory name with a space": func(t *testing.T, subDir string) {
			subCommitFile(t, subDir, key+" dir/x.txt", attackSecretLine(), "adds")
		},
		"key in the commit message only": func(t *testing.T, subDir string) {
			subCommitFile(t, subDir, "a.txt", "innocent\n", "message holds "+key)
		},
		"key in a binary file": func(t *testing.T, subDir string) {
			subCommitFile(t, subDir, "b.bin", "\x00"+attackSecretLine(), "binary")
		},
	} {
		t.Run(name, func(t *testing.T) {
			root, subDir := submoduleRepoWith(t, map[string]string{"s.txt": "ok\n"})
			build(t, subDir)
			o := attackRun(t, root, "TASK-ATK2-LEAK", dc.PlannedFile{Path: "sub", AllowedChange: dc.ChangeModify})
			noKeyAnywhere(t, key, o)
			requireGitlink(t, key, o, "sub", "moved")
		})
	}
}

// ---------------------------------------------------------------- history rewrites

// A shallow file, a graft, a forged commit-graph or a missing tree object each
// change what git would show of the submodule's history. None of them changes
// that the gitlink moved, which is all the run reads.
func TestGitlinkHistoryRewritesStillDegrade(t *testing.T) {
	key := exampleAWSKey()
	secretThenRemoved := func(t *testing.T) (root, sub, old, bad, tip string) {
		root, sub = submoduleRepoWith(t, map[string]string{"s.txt": "ok\n"})
		old = gitOutIn(t, sub, "rev-parse", "HEAD")
		subCommitFile(t, sub, "keys.txt", attackSecretLine(), "B adds the secret")
		bad = gitOutIn(t, sub, "rev-parse", "HEAD")
		runGitIn(t, sub, "rm", "-q", "keys.txt")
		runGitIn(t, sub, "commit", "-qm", "C removes it")
		tip = gitOutIn(t, sub, "rev-parse", "HEAD")
		return root, sub, old, bad, tip
	}
	gitDir := func(t *testing.T, sub string) string {
		return gitOutIn(t, sub, "rev-parse", "--absolute-git-dir")
	}
	cases := map[string]func(t *testing.T) string{
		"a shallow file cutting the walk at the tip": func(t *testing.T) string {
			root, sub, _, _, tip := secretThenRemoved(t)
			if err := os.WriteFile(filepath.Join(gitDir(t, sub), "shallow"), []byte(tip+"\n"), 0o644); err != nil {
				t.Fatal(err)
			}
			return root
		},
		"a graft giving the tip another parent": func(t *testing.T) string {
			root, sub, old, _, tip := secretThenRemoved(t)
			info := filepath.Join(gitDir(t, sub), "info")
			if err := os.MkdirAll(info, 0o755); err != nil {
				t.Fatal(err)
			}
			if err := os.WriteFile(filepath.Join(info, "grafts"), []byte(tip+" "+old+"\n"), 0o644); err != nil {
				t.Fatal(err)
			}
			return root
		},
		"a commit-graph that lies about a parent": func(t *testing.T) string {
			root, sub, _, _, tip := secretThenRemoved(t)
			runGitIn(t, sub, "commit-graph", "write", "--reachable")
			path := filepath.Join(gitDir(t, sub), "objects", "info", "commit-graph")
			data, err := os.ReadFile(path)
			if err != nil {
				t.Skipf("no commit-graph written: %v", err)
			}
			if !lieAboutParent(t, data, tip, path) {
				t.Skip("could not locate the commit in the graph")
			}
			return root
		},
		"a missing tree object": func(t *testing.T) string {
			root, sub, _, bad, _ := secretThenRemoved(t)
			tree := gitOutIn(t, sub, "rev-parse", bad+"^{tree}")
			if err := os.Remove(filepath.Join(gitDir(t, sub), "objects", tree[:2], tree[2:])); err != nil {
				t.Skipf("the tree object of B is not loose: %v", err)
			}
			return root
		},
		"a forged loose commit object": func(t *testing.T) string {
			root, sub, _, bad, _ := secretThenRemoved(t)
			path := filepath.Join(gitDir(t, sub), "objects", bad[:2], bad[2:])
			var raw bytes.Buffer
			zw := zlib.NewWriter(&raw)
			content := "tree " + gitOutIn(t, sub, "rev-parse", bad+"^{tree}") + "\n\nforged\n"
			if _, err := zw.Write([]byte("commit " + strconv.Itoa(len(content)) + "\x00" + content)); err != nil {
				t.Fatal(err)
			}
			if err := zw.Close(); err != nil {
				t.Fatal(err)
			}
			if err := os.Chmod(path, 0o644); err != nil {
				t.Skipf("the commit object of B is not loose: %v", err)
			}
			if err := os.WriteFile(path, raw.Bytes(), 0o644); err != nil {
				t.Fatal(err)
			}
			return root
		},
	}
	for name, build := range cases {
		t.Run(name, func(t *testing.T) {
			root := build(t)
			o := attackRun(t, root, "TASK-GL-REWRITE", dc.PlannedFile{Path: "sub", AllowedChange: dc.ChangeModify})
			requireGitlink(t, key, o, "sub", "moved")
			noKeyAnywhere(t, key, o)
		})
	}
}

// lieAboutParent rewrites path so the commit-graph says `commit` has no parent,
// and fixes the trailing checksum.
func lieAboutParent(t *testing.T, data []byte, commit, path string) bool {
	t.Helper()
	if len(data) < 8+12 || string(data[:4]) != "CGPH" {
		return false
	}
	chunks := int(data[6])
	type chunk struct{ off, end int }
	found := map[string]chunk{}
	for i := 0; i < chunks; i++ {
		e := data[8+i*12:]
		found[string(e[:4])] = chunk{int(binary.BigEndian.Uint64(e[4:12])), int(binary.BigEndian.Uint64(e[16:24]))}
	}
	oidl, cdat := found["OIDL"], found["CDAT"]
	if oidl.end == 0 || cdat.end == 0 {
		return false
	}
	want := make([]byte, 20)
	for i := range want {
		v, err := strconv.ParseUint(commit[i*2:i*2+2], 16, 8)
		if err != nil {
			return false
		}
		want[i] = byte(v)
	}
	n := (oidl.end - oidl.off) / 20
	for i := 0; i < n; i++ {
		if bytes.Equal(data[oidl.off+i*20:oidl.off+i*20+20], want) {
			entry := cdat.off + i*36
			binary.BigEndian.PutUint32(data[entry+20:], 0x70000000)
			sum := sha1.Sum(data[:len(data)-20]) // #nosec G401
			copy(data[len(data)-20:], sum[:])
			if err := os.Chmod(path, 0o644); err != nil {
				t.Fatal(err)
			}
			if err := os.WriteFile(path, data, 0o644); err != nil {
				t.Fatal(err)
			}
			return true
		}
	}
	return false
}

// A listing of gitlinks past what one read examines is refused, never sampled.
func TestGitlinkMoreThanTheBoundIsRefusedNotSampled(t *testing.T) {
	root := gitRepo(t)
	var index bytes.Buffer
	for i := 0; i < 257; i++ {
		fmt.Fprintf(&index, "160000 %s 0\tm%03d\n", strings.Repeat("ab", 20), i)
	}
	cmd := exec.Command(testsupport.Tool(t, "git"), "update-index", "--add", "--index-info")
	cmd.Dir = root
	cmd.Stdin = &index
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("update-index: %v %s", err, out)
	}
	o := attackRun(t, root, "TASK-GL-MANY")
	if o.refused == nil {
		t.Fatalf("257 gitlinks were collected: status=%s degraded=%q", o.result.Status, o.result.DegradedReasons)
	}
}
