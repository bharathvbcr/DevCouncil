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

func TestADirtySubmoduleFileIsScanned(t *testing.T) {
	key := exampleAWSKey()
	git := testsupport.Tool(t, "git")
	root := gitRepo(t)
	src := filepath.Join(t.TempDir(), "sub-src")
	run := func(dir string, args ...string) {
		t.Helper()
		cmd := exec.Command(git, args...)
		cmd.Dir = dir
		cmd.Env = append(os.Environ(),
			"GIT_AUTHOR_NAME=t", "GIT_AUTHOR_EMAIL=t@example.invalid",
			"GIT_COMMITTER_NAME=t", "GIT_COMMITTER_EMAIL=t@example.invalid",
			"GIT_CONFIG_NOSYSTEM=1", "GIT_CONFIG_GLOBAL="+os.DevNull)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v in %s: %v %s", args, dir, err, out)
		}
	}
	if err := os.MkdirAll(src, 0o755); err != nil {
		t.Fatal(err)
	}
	run(src, "init", "-q")
	writeFile(t, src, "s.txt", "ok\n")
	run(src, "add", "s.txt")
	run(src, "commit", "-qm", "s")
	run(root, "-c", "protocol.file.allow=always", "submodule", "add", "-q", src, "sub")
	run(root, "commit", "-qm", "sub")
	writeFile(t, filepath.Join(root, "sub"), "s.txt", "let aws_key = \""+key+"\";\n")

	changed, diff, empty, err := verify.CollectGitState(context.Background(), root)
	if err != nil {
		t.Fatal(err)
	}
	if empty {
		t.Fatal("dirty submodule collected as an empty tree")
	}
	if strings.Contains(diff, key) {
		// The bytes were included, which is what the scanner needs. The
		// report below must still not quote them.
	}
	in := verify.Input{
		Task: &store.Task{
			ID: "TASK-SUB", Difficulty: "easy",
			PlannedFiles: []dc.PlannedFile{{Path: "sub/s.txt", AllowedChange: dc.ChangeModify}},
		},
		GateMode: "enforce", ChangedFiles: changed, DiffContent: diff, DiffEmpty: empty,
		WorkPresent: !empty, Rigor: dcverify.New(testsupport.DCVerify(t), root),
	}
	gaps, meta := verify.Run(context.Background(), in)
	result := verify.ToMCP("TASK-SUB", gaps, meta)
	reportLeaks(t, result, gaps, key)
	if result.RigorSkippedReason != "" {
		t.Fatalf("rigor skipped: %q", result.RigorSkippedReason)
	}
	if result.Passed || result.Status == "verified" {
		t.Fatalf("status=%s passed=%v", result.Status, result.Passed)
	}
	found := false
	for _, g := range gaps {
		if g.GapType == "security_risk" || g.GapType == "rigor_check_unavailable" {
			found = true
		}
	}
	if !found {
		t.Fatal("dirty submodule bytes produced neither a secret finding nor an unavailable gap")
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
