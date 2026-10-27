package verify_test

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/dcverify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// exampleAWSKey is the access-key id AWS publishes as an example. It is
// split so a failure message that prints a joined string is the only way it
// would be reassembled, and the assertions below never print a gap that
// contains it.
func exampleAWSKey() string { return "AKIA" + "IOSFODNN7EXAMPLE" }

func gitRepo(t *testing.T) string {
	t.Helper()
	git := testsupport.Tool(t, "git")
	root := t.TempDir()
	run := func(args ...string) {
		t.Helper()
		cmd := exec.Command(git, args...)
		cmd.Dir = root
		cmd.Env = append(os.Environ(),
			"GIT_AUTHOR_NAME=t", "GIT_AUTHOR_EMAIL=t@example.invalid",
			"GIT_COMMITTER_NAME=t", "GIT_COMMITTER_EMAIL=t@example.invalid",
			"GIT_CONFIG_NOSYSTEM=1", "GIT_CONFIG_GLOBAL="+os.DevNull)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v %s", args, err, out)
		}
	}
	run("init", "-q", root)
	if err := os.WriteFile(filepath.Join(root, "README"), []byte("base\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("add", "README")
	run("commit", "-qm", "base")
	return root
}

func writeFile(t *testing.T, root, rel, body string) {
	t.Helper()
	full := filepath.Join(root, filepath.FromSlash(rel))
	if err := os.MkdirAll(filepath.Dir(full), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(full, []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
}

func reportLeaks(t *testing.T, result verify.MCPResult, gaps []verify.Gap, secret string) {
	t.Helper()
	blob := result.RigorSkippedReason + " " + result.CoverageSkippedReason + " " + strings.Join(result.DegradedReasons, " ")
	for _, g := range gaps {
		blob += " " + g.Description + " " + g.GapType + " " + strings.Join(g.Evidence, " ")
	}
	if strings.Contains(blob, secret) {
		t.Fatal("a gap, skip reason, or degraded reason contains the credential")
	}
}

// TestABlankDiffWithAnUntrackedFileIsNotVerified. git diff HEAD is empty
// when the only change is untracked. That used to skip the gates and verify.
func TestABlankDiffWithAnUntrackedFileIsNotVerified(t *testing.T) {
	key := exampleAWSKey()
	body := "pub fn f() {\n    let aws_key = \"" + key + "\";\n    todo!()\n}\n"
	for _, mode := range []string{"advisory", "enforce"} {
		t.Run(mode, func(t *testing.T) {
			root := gitRepo(t)
			writeFile(t, root, "src/new.rs", body)
			changed, diff, empty, err := verify.CollectGitState(context.Background(), root)
			if err != nil {
				t.Fatal(err)
			}
			if empty || len(changed) == 0 {
				t.Fatalf("untracked file was not a change: empty=%v changed=%v", empty, changed)
			}
			in := verify.Input{
				Task: &store.Task{
					ID: "TASK-UNTRACKED", Difficulty: "easy",
					PlannedFiles: []dc.PlannedFile{{Path: "src/new.rs", AllowedChange: dc.ChangeCreate}},
				},
				GateMode: mode, ChangedFiles: changed, DiffContent: diff, DiffEmpty: empty,
				WorkPresent: !empty, Rigor: dcverify.New(testsupport.DCVerify(t), root),
			}
			gaps, meta := verify.Run(context.Background(), in)
			result := verify.ToMCP("TASK-UNTRACKED", gaps, meta)
			reportLeaks(t, result, gaps, key)
			if result.RigorSkippedReason != "" {
				t.Fatalf("%s: rigor skipped (%q) for a named untracked file", mode, result.RigorSkippedReason)
			}
			if result.Passed || result.Status == "verified" || result.Status == "skipped" {
				t.Fatalf("%s: status=%s passed=%v", mode, result.Status, result.Passed)
			}
			found := false
			for _, g := range gaps {
				if g.GapType == "security_risk" || g.GapType == "stub_detected" || g.GapType == "rigor_check_unavailable" {
					found = true
				}
			}
			if !found {
				t.Fatalf("%s: no secret, stub, or unavailable gap", mode)
			}
		})
	}
}

func TestAnUntrackedSymlinkIsNotFollowed(t *testing.T) {
	key := exampleAWSKey()
	root := gitRepo(t)
	outside := filepath.Join(t.TempDir(), "secret.txt")
	if err := os.WriteFile(outside, []byte(key), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(outside, filepath.Join(root, "link.rs")); err != nil {
		t.Fatal(err)
	}
	_, _, _, err := verify.CollectGitState(context.Background(), root)
	if err == nil {
		t.Fatal("a symlink was included, or the untracked name was ignored")
	}
	if strings.Contains(err.Error(), key) {
		t.Fatal("the error quotes the symlink target's contents")
	}
	in := verify.Input{
		Task:     &store.Task{ID: "TASK-LINK", Difficulty: "easy"},
		GateMode: "advisory", GitErr: err, DiffEmpty: true, WorkPresent: false,
		Rigor: dcverify.New(testsupport.DCVerify(t), root),
	}
	gaps, meta := verify.Run(context.Background(), in)
	result := verify.ToMCP("TASK-LINK", gaps, meta)
	reportLeaks(t, result, gaps, key)
	if result.Passed || result.Status == "verified" {
		t.Fatalf("status=%s passed=%v", result.Status, result.Passed)
	}
	found := false
	for _, g := range gaps {
		if g.GapType == "rigor_check_unavailable" && g.Blocking {
			found = true
		}
	}
	if !found {
		t.Fatal("symlink was not a blocking rigor_check_unavailable")
	}
}

func runGitIn(t *testing.T, dir string, args ...string) {
	t.Helper()
	cmd := exec.Command(testsupport.Tool(t, "git"), args...)
	cmd.Dir = dir
	cmd.Env = append(os.Environ(),
		"GIT_AUTHOR_NAME=t", "GIT_AUTHOR_EMAIL=t@example.invalid",
		"GIT_COMMITTER_NAME=t", "GIT_COMMITTER_EMAIL=t@example.invalid",
		"GIT_CONFIG_NOSYSTEM=1", "GIT_CONFIG_GLOBAL="+os.DevNull)
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("git %v in %s: %v %s", args, dir, err, out)
	}
}

// submoduleRepo returns a repository whose committed submodule `sub` holds
// s.txt and keep.txt, both clean.
func submoduleRepo(t *testing.T) (root, subDir string) {
	t.Helper()
	root = gitRepo(t)
	src := filepath.Join(t.TempDir(), "sub-src")
	if err := os.MkdirAll(src, 0o755); err != nil {
		t.Fatal(err)
	}
	runGitIn(t, src, "init", "-q")
	writeFile(t, src, "s.txt", "ok\n")
	writeFile(t, src, "keep.txt", "keep\n")
	runGitIn(t, src, "add", "s.txt", "keep.txt")
	runGitIn(t, src, "commit", "-qm", "s")
	runGitIn(t, root, "-c", "protocol.file.allow=always", "submodule", "add", "-q", src, "sub")
	runGitIn(t, root, "commit", "-qm", "sub")
	return root, filepath.Join(root, "sub")
}

// verifyPlanned collects the tree and runs the gates over it with the given
// planned paths, the way the host does.
func verifyPlanned(t *testing.T, root, id string, planned ...dc.PlannedFile) ([]verify.Gap, verify.MCPResult) {
	t.Helper()
	changed, diff, empty, err := verify.CollectGitState(context.Background(), root)
	if err != nil {
		t.Fatalf("CollectGitState: %v", err)
	}
	if empty {
		t.Fatal("the change was collected as an empty tree")
	}
	in := verify.Input{
		Task:     &store.Task{ID: id, Difficulty: "easy", PlannedFiles: planned},
		GateMode: "enforce", ChangedFiles: changed, DiffContent: diff, DiffEmpty: empty,
		WorkPresent: !empty, Rigor: dcverify.New(testsupport.DCVerify(t), root),
	}
	gaps, meta := verify.Run(context.Background(), in)
	return gaps, verify.ToMCP(id, gaps, meta)
}

// requireSecretFinding fails unless the run is not verified and carries a
// security_risk gap: the credential was read and found. An
// unavailable-rigor gap is a different fact and is not accepted here — that is
// "could not look", and the point of the test is that it looked.
func requireSecretFinding(t *testing.T, result verify.MCPResult, gaps []verify.Gap, key string) {
	t.Helper()
	reportLeaks(t, result, gaps, key)
	if result.RigorSkippedReason != "" {
		t.Fatalf("rigor skipped: %q", result.RigorSkippedReason)
	}
	if result.Passed || result.Status == "verified" {
		t.Fatalf("status=%s passed=%v", result.Status, result.Passed)
	}
	for _, g := range gaps {
		if g.GapType == "security_risk" {
			return
		}
	}
	t.Fatal("the credential in the submodule file was not found: no security_risk gap")
}

func TestADirtySubmoduleFileIsScanned(t *testing.T) {
	key := exampleAWSKey()
	secret := "let aws_key = \"" + key + "\";\n"
	planned := []dc.PlannedFile{
		{Path: "sub", AllowedChange: dc.ChangeModify},
		{Path: "sub/s.txt", AllowedChange: dc.ChangeModify},
	}

	t.Run("worktree only", func(t *testing.T) {
		root, subDir := submoduleRepo(t)
		writeFile(t, subDir, "s.txt", secret)
		_, diff, _, err := verify.CollectGitState(context.Background(), root)
		if err != nil {
			t.Fatal(err)
		}
		if !strings.Contains(diff, key) {
			t.Fatal("the submodule file's bytes were not read into the diff")
		}
		gaps, result := verifyPlanned(t, root, "TASK-SUB", planned...)
		requireSecretFinding(t, result, gaps, key)
	})

	// The submodule's checked-out commit moved, so the superproject diff already
	// carries a header for `sub` (a Subproject commit hunk). The uncommitted
	// bytes inside it are still unscanned unless that header is ignored.
	t.Run("pointer moved", func(t *testing.T) {
		root, subDir := submoduleRepo(t)
		writeFile(t, subDir, "keep.txt", "keep two\n")
		runGitIn(t, subDir, "commit", "-qam", "move the pointer")
		writeFile(t, subDir, "s.txt", secret)
		_, diff, _, err := verify.CollectGitState(context.Background(), root)
		if err != nil {
			t.Fatal(err)
		}
		if !strings.Contains(diff, "Subproject commit") {
			t.Fatal("fixture did not move the submodule pointer")
		}
		if !strings.Contains(diff, key) {
			t.Fatal("the submodule file's uncommitted bytes were not read into the diff")
		}
		gaps, result := verifyPlanned(t, root, "TASK-SUB-MOVED", planned...)
		requireSecretFinding(t, result, gaps, key)
	})
}

// A submodule whose only change is a deleted file has no bytes to scan. That
// is a change with nothing to read, not a read that failed.
func TestADeletionOnlySubmoduleIsNotAnUnreadableRead(t *testing.T) {
	root, subDir := submoduleRepo(t)
	if err := os.Remove(filepath.Join(subDir, "s.txt")); err != nil {
		t.Fatal(err)
	}
	gaps, result := verifyPlanned(t, root, "TASK-SUB-DEL",
		dc.PlannedFile{Path: "sub", AllowedChange: dc.ChangeModify},
		dc.PlannedFile{Path: "sub/s.txt", AllowedChange: dc.ChangeDelete})
	for _, g := range gaps {
		if g.GapType == "rigor_check_unavailable" {
			t.Fatalf("a deletion-only submodule change blocked as an unreadable read: %s", g.Description)
		}
	}
	if result.RigorSkippedReason != "" {
		t.Fatalf("rigor skipped: %q", result.RigorSkippedReason)
	}
}

func TestASecretPathIsScannedAndWithheld(t *testing.T) {
	key := exampleAWSKey()
	root := gitRepo(t)
	writeFile(t, root, "secrets/prod.rs", "let aws_key = \""+key+"\";\n")
	changed, diff, empty, err := verify.CollectGitState(context.Background(), root)
	if err != nil {
		t.Fatal(err)
	}
	in := verify.Input{
		Task: &store.Task{
			ID: "TASK-SECRETPATH", Difficulty: "easy",
			PlannedFiles: []dc.PlannedFile{{Path: "secrets/prod.rs", AllowedChange: dc.ChangeCreate}},
		},
		GateMode: "enforce", ChangedFiles: changed, DiffContent: diff, DiffEmpty: empty,
		WorkPresent: !empty, Rigor: dcverify.New(testsupport.DCVerify(t), root),
	}
	gaps, meta := verify.Run(context.Background(), in)
	result := verify.ToMCP("TASK-SECRETPATH", gaps, meta)
	reportLeaks(t, result, gaps, key)
	if result.Passed || result.Status == "verified" {
		t.Fatalf("status=%s passed=%v", result.Status, result.Passed)
	}
	withheld := false
	for _, g := range gaps {
		if g.GapType == "security_risk" {
			for _, e := range g.Evidence {
				if strings.Contains(e, "withheld: secret_path") {
					withheld = true
				}
			}
		}
	}
	if !withheld {
		t.Fatal("secret-path evidence was not withheld")
	}
}

func TestModeOffOnAnEmptyTreeStaysSkipped(t *testing.T) {
	root := gitRepo(t)
	changed, diff, empty, err := verify.CollectGitState(context.Background(), root)
	if err != nil {
		t.Fatal(err)
	}
	if !empty || len(changed) != 0 || strings.TrimSpace(diff) != "" {
		t.Fatalf("empty tree collected as changed=%v empty=%v", changed, empty)
	}
	in := verify.Input{
		Task:         &store.Task{ID: "TASK-EMPTY", Difficulty: "easy"},
		GateMode:     "off",
		ChangedFiles: changed,
		DiffContent:  diff,
		DiffEmpty:    empty,
		WorkPresent:  !empty,
		Rigor:        dcverify.New(testsupport.DCVerify(t), root),
	}
	gaps, meta := verify.Run(context.Background(), in)
	result := verify.ToMCP("TASK-EMPTY", gaps, meta)
	if result.Status != "skipped" || result.Passed {
		t.Fatalf("empty tree mode off: status=%s passed=%v", result.Status, result.Passed)
	}
	if result.RigorSkippedReason == "" {
		t.Fatal("empty tree reported rigor without saying it was skipped")
	}
	if result.CoverageSkippedReason != "no diff to measure" {
		t.Fatalf("coverage_skipped_reason=%q", result.CoverageSkippedReason)
	}
}

func TestANamedChangeWithABlankDiffIsNotASkip(t *testing.T) {
	for _, mode := range []string{"advisory", "enforce"} {
		in := verify.Input{
			Task: &store.Task{
				ID: "TASK-BLANK", Difficulty: "easy",
				PlannedFiles: []dc.PlannedFile{{Path: "src/a.rs", AllowedChange: dc.ChangeModify}},
			},
			GateMode:     mode,
			ChangedFiles: []string{"src/a.rs"},
			DiffContent:  "",
			DiffEmpty:    false,
			WorkPresent:  true,
			Rigor:        dcverify.New(testsupport.DCVerify(t), t.TempDir()),
		}
		gaps, meta := verify.Run(context.Background(), in)
		result := verify.ToMCP("TASK-BLANK", gaps, meta)
		if result.RigorSkippedReason != "" {
			t.Fatalf("%s: blank diff with a name list skipped: %q", mode, result.RigorSkippedReason)
		}
		if result.Passed || result.Status == "verified" {
			t.Fatalf("%s: status=%s passed=%v", mode, result.Status, result.Passed)
		}
		found := false
		for _, g := range gaps {
			if g.GapType == "rigor_check_unavailable" && g.Blocking {
				found = true
			}
		}
		if !found {
			t.Fatalf("%s: missing blocking rigor_check_unavailable", mode)
		}
	}
}
