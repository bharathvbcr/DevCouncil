package stopgate_test

// The production path, end to end, with nothing stubbed.
//
// verify's own tests hand verify.Run a client they constructed. That proves the
// translation from findings to gaps, and it proves nothing about whether the
// host ever builds such a client — which was the entire content of
// GAP-P7-DCVERIFY-UNWIRED. VerifyTask resolves the binary itself, through
// RigorClient, from inside stopgate.Run; a wiring that stopped there would
// leave every test in verify green and the shipped host exactly as unwired as
// before.
//
// So this drives the real store, a real git repository with a real credential
// in its working tree, and the real dcverify — and asks the one question that
// matters: does the host block it?

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/dcverify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/stopgate"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// repoWithWorkingTreeChange builds a git repository whose diff against HEAD is
// the given file content. The commit is necessary: CollectGitState reads
// `git diff HEAD`, and an untracked file produces a file list with no diff for
// the gates to read.
func repoWithWorkingTreeChange(t *testing.T, path, content string) string {
	t.Helper()
	git := testsupport.Tool(t, "git")
	root := t.TempDir()

	run := func(args ...string) {
		t.Helper()
		cmd := exec.Command(git, args...)
		cmd.Dir = root
		// A repository with no identity cannot commit, and the machine's own
		// git config is not this test's to depend on.
		cmd.Env = append(os.Environ(),
			"GIT_AUTHOR_NAME=t", "GIT_AUTHOR_EMAIL=t@example.invalid",
			"GIT_COMMITTER_NAME=t", "GIT_COMMITTER_EMAIL=t@example.invalid")
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v %s", args, err, out)
		}
	}
	run("init", "-q", root)
	full := filepath.Join(root, path)
	if err := os.MkdirAll(filepath.Dir(full), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(full, []byte("// placeholder\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	run("add", ".")
	run("commit", "-qm", "base")
	if err := os.WriteFile(full, []byte(content), 0o600); err != nil {
		t.Fatal(err)
	}
	return root
}

// plantTask inserts one ready task with the given planned file.
func plantTask(t *testing.T, client *store.Client, id, plannedFile string) {
	t.Helper()
	planned := `[{"path":"` + plannedFile + `","allowed_change":"modify"}]`
	sql := "INSERT INTO tasks (id,title,description,planned_files_json,expected_tests_json,status) " +
		"VALUES ('" + id + "','rigor','','" + planned + "','[]','ready');"
	if out, err := exec.Command(testsupport.Tool(t, "sqlite3"), client.DB, sql).CombinedOutput(); err != nil {
		t.Fatalf("task fixture: %v %s", err, out)
	}
}

func newStore(t *testing.T) *store.Client {
	t.Helper()
	client := store.New(testsupport.DCStore(t), filepath.Join(t.TempDir(), "state.sqlite"))
	// Forces schema creation, the way the sibling store tests do.
	if _, err := client.ActiveLeases(context.Background()); err != nil {
		t.Fatal(err)
	}
	return client
}

// TestTheShippedHostBlocksACredentialInTheWorkingTree is GAP-P7-DCVERIFY-UNWIRED
// stated as a behaviour rather than as a wiring diagram.
//
// The key is `AKIAIOSFODNN7EXAMPLE`, the example key AWS publishes for this
// purpose. Before this change the host verified this working tree clean:
// `verify.Run` recorded `rigor_applied: []` and never spawned a verifier, so
// nothing in the pipeline had looked at the added line at all.
func TestTheShippedHostBlocksACredentialInTheWorkingTree(t *testing.T) {
	// The host resolves dcverify off PATH, excluding candidates inside the
	// repository under analysis. In this checkout the binary lives under the
	// cargo target directory, so the override is how a test points at the build
	// it just made rather than at whatever happens to be installed.
	t.Setenv(dcverify.BinaryEnv, testsupport.DCVerify(t))

	root := repoWithWorkingTreeChange(t, "src/handler.go",
		"// placeholder\nconst awsKey = \"AKIAIOSFODNN7EXAMPLE\"\n")
	client := newStore(t)
	plantTask(t, client, "RIGOR-E2E", "src/handler.go")

	out := stopgate.Run(context.Background(), stopgate.Input{
		Root: root, Store: client, TaskID: "RIGOR-E2E", GateMode: "enforce",
	})

	if out.Decision.Skipped {
		t.Fatalf("verification skipped: %+v", out.Decision)
	}
	if len(out.MCP.RigorApplied) == 0 {
		t.Fatalf("the shipped host ran no rigor gates: rigor_applied=%v reason=%q",
			out.MCP.RigorApplied, out.MCP.RigorSkippedReason)
	}

	found := false
	for _, gap := range out.Gaps {
		if gap.GapType == "security_risk" {
			found = true
			if !gap.Blocking {
				t.Error("a credential in the diff must block")
			}
			for _, e := range gap.Evidence {
				if strings.Contains(e, "AKIAIOSFODNN7EXAMPLE") {
					t.Errorf("the gap quotes the credential in full: %q", e)
				}
			}
		}
	}
	if !found {
		t.Fatalf("no security_risk gap for a diff containing a credential: %+v", out.Gaps)
	}
	if out.Decision.Allow || out.MCP.Passed {
		t.Fatalf("the host allowed a change carrying a credential: %+v", out)
	}
}

// TestACleanWorkingTreeStillVerifies is the positive control, and it is not
// optional: a rigor layer that blocks everything is indistinguishable from one
// that works, and far easier to write by accident.
func TestACleanWorkingTreeStillVerifies(t *testing.T) {
	t.Setenv(dcverify.BinaryEnv, testsupport.DCVerify(t))

	root := repoWithWorkingTreeChange(t, "src/handler.go",
		"// placeholder\nconst answer = 42\n")
	client := newStore(t)
	plantTask(t, client, "RIGOR-E2E-CLEAN", "src/handler.go")

	out := stopgate.Run(context.Background(), stopgate.Input{
		Root: root, Store: client, TaskID: "RIGOR-E2E-CLEAN", GateMode: "enforce",
	})

	for _, gap := range out.Gaps {
		if gap.GapType == "security_risk" || gap.GapType == "stub_detected" ||
			gap.GapType == "rigor_check_unavailable" {
			t.Errorf("clean diff earned %s: %+v", gap.GapType, gap)
		}
	}
	// The gates ran and found nothing — which is only meaningful because
	// rigor_applied says they ran.
	if len(out.MCP.RigorApplied) == 0 {
		t.Fatalf("rigor_applied=%v reason=%q", out.MCP.RigorApplied, out.MCP.RigorSkippedReason)
	}
	if !out.MCP.Passed {
		t.Fatalf("a clean in-scope change must verify: %+v", out.MCP)
	}
}

// TestAHostWithNoVerifierSaysSoRatherThanVerifyingClean is the same credential,
// through the same host, with dcverify unreachable.
//
// It must not pass — but it must also not silently pass: the result carries a
// reason an operator can act on, which is the difference between "we looked and
// found nothing" and "nothing looked".
func TestAHostWithNoVerifierSaysSoRatherThanVerifyingClean(t *testing.T) {
	root := repoWithWorkingTreeChange(t, "src/handler.go",
		"// placeholder\nconst awsKey = \"AKIAIOSFODNN7EXAMPLE\"\n")
	client := newStore(t)
	plantTask(t, client, "RIGOR-E2E-ABSENT", "src/handler.go")

	// A PATH carrying git and nothing else, so discovery genuinely finds no
	// verifier while the diff is still collectable. Emptying PATH outright
	// would take git with it, CollectGitState would report no work, and the
	// gates would skip for having nothing to read — a green test about a
	// different skip than the one it names.
	onlyGit := t.TempDir()
	if err := os.Symlink(testsupport.Tool(t, "git"), filepath.Join(onlyGit, "git")); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", onlyGit)
	t.Setenv(dcverify.BinaryEnv, "")

	out := stopgate.Run(context.Background(), stopgate.Input{
		Root: root, Store: client, TaskID: "RIGOR-E2E-ABSENT", GateMode: "enforce",
	})

	if len(out.MCP.RigorApplied) != 0 {
		t.Errorf("rigor_applied=%v with no verifier on PATH", out.MCP.RigorApplied)
	}
	if out.MCP.RigorSkippedReason == "" {
		t.Fatal("the host scanned nothing and said nothing about it")
	}
	if !strings.Contains(out.MCP.RigorSkippedReason, "dcverify") {
		t.Errorf("rigor_skipped_reason=%q does not name what is missing", out.MCP.RigorSkippedReason)
	}
}
