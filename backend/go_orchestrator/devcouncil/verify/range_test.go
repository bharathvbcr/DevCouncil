package verify

import (
	"context"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/dcverify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
)

func initTestRepo(t *testing.T) (string, func(args ...string) string) {
	t.Helper()
	dir := t.TempDir()
	runGit := func(args ...string) string {
		t.Helper()
		cmd := exec.Command("git", args...)
		cmd.Dir = dir
		cmd.Env = append(os.Environ(),
			"GIT_AUTHOR_NAME=test",
			"GIT_AUTHOR_EMAIL=test@example.com",
			"GIT_COMMITTER_NAME=test",
			"GIT_COMMITTER_EMAIL=test@example.com",
		)
		out, err := cmd.CombinedOutput()
		if err != nil {
			t.Fatalf("git %v failed: %v\n%s", args, err, out)
		}
		return strings.TrimSpace(string(out))
	}
	runGit("init", "-b", "main")
	return dir, runGit
}

func writeFile(t *testing.T, root, rel, content string) {
	t.Helper()
	full := filepath.Join(root, filepath.FromSlash(rel))
	if err := os.MkdirAll(filepath.Dir(full), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(full, []byte(content), 0o644); err != nil {
		t.Fatal(err)
	}
}

func fakeDCVerify(t *testing.T, jsonReply string) *dcverify.Client {
	t.Helper()
	if runtime.GOOS == "windows" {
		t.Skip("shell script fake requires unix")
	}
	bin := filepath.Join(t.TempDir(), "fake-dcverify")
	body := "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '" + jsonReply + "'\n"
	if err := os.WriteFile(bin, []byte(body), 0o755); err != nil {
		t.Fatal(err)
	}
	c := dcverify.New(bin, t.TempDir())
	c.Timeout = 5 * time.Second
	return c
}

const cleanDCVerifyJSON = `{"ok":true,"files":1,"in_scope":[],"orphans":["app.go"],"untouched_planned":[],"findings":[],"coverage_unmeasured":["app.go"],"coverage_gaps":[],"coverage_skipped_by_type":[],"stub_ast_parsed_files":1,"runner_hijack_checked":true,"lean_tcb_checked":false,"stub_ast_degraded":[],"substance":{"added_lines":2,"substantive_lines":2,"trivial":0,"moved":0,"repeated":0,"generated":0,"judged":false,"low":false,"files":[{"path":"app.go","added_lines":2,"substantive_lines":2}]}}`

func TestCollectRangeStateDetectsGitattributesDiffSuppression(t *testing.T) {
	dir, git := initTestRepo(t)
	// Base commit already sets *.go -diff so a PR modifying app.go tries to hide added lines.
	writeFile(t, dir, ".gitattributes", "*.go -diff\n")
	writeFile(t, dir, "app.go", "package app\nfunc A() int { return 1 }\n")
	git("add", ".")
	git("commit", "-m", "base")
	base := git("rev-parse", "HEAD")

	writeFile(t, dir, "app.go", "package app\nfunc A() int { panic(\"todo\") }\n")
	git("add", ".")
	git("commit", "-m", "head")
	head := git("rev-parse", "HEAD")

	state, err := CollectRangeState(context.Background(), dir, base, head)
	if err != nil {
		t.Fatalf("CollectRangeState: %v", err)
	}
	if len(state.BinarySuppressed) != 1 || state.BinarySuppressed[0] != "app.go" {
		t.Fatalf("BinarySuppressed = %v, want [app.go]", state.BinarySuppressed)
	}
	if !strings.Contains(state.Diff, `+func A() int { panic("todo") }`) {
		t.Fatalf("expected --text diff to expose hidden added line, got:\n%s", state.Diff)
	}
}

func TestCollectRangeStateClassifiesSensitiveAndDependencyPaths(t *testing.T) {
	dir, git := initTestRepo(t)
	writeFile(t, dir, "README.md", "hi\n")
	git("add", ".")
	git("commit", "-m", "base")
	base := git("rev-parse", "HEAD")

	writeFile(t, dir, ".gitattributes", "* text=auto\n")
	writeFile(t, dir, ".gitmodules", "[submodule \"sub\"]\n\tpath = sub\n")
	writeFile(t, dir, ".github/workflows/ci.yml", "name: ci\n")
	writeFile(t, dir, ".github/actions/helper/run.sh", "#!/bin/sh\n")
	writeFile(t, dir, ".devcouncil/config.yaml", "gates:\n  mode: off\n")
	writeFile(t, dir, "action.yml", "name: gate\n")
	writeFile(t, dir, "nested/action.yaml", "name: nested\n")
	writeFile(t, dir, "go.mod", "module example.com/m\n")
	git("add", ".")
	git("commit", "-m", "head")
	head := git("rev-parse", "HEAD")

	state, err := CollectRangeState(context.Background(), dir, base, head)
	if err != nil {
		t.Fatalf("CollectRangeState: %v", err)
	}
	wantSensitive := []string{
		".devcouncil/config.yaml",
		".gitattributes",
		".github/actions/helper/run.sh",
		".github/workflows/ci.yml",
		".gitmodules",
		"action.yml",
		"nested/action.yaml",
	}
	if strings.Join(state.SensitiveFiles, ",") != strings.Join(wantSensitive, ",") {
		t.Fatalf("SensitiveFiles = %v, want %v", state.SensitiveFiles, wantSensitive)
	}
	if len(state.DependencyFiles) != 1 || state.DependencyFiles[0] != "go.mod" {
		t.Fatalf("DependencyFiles = %v, want [go.mod]", state.DependencyFiles)
	}
}

func TestCollectRangeStateRefusesDirtyTrackedWorktree(t *testing.T) {
	dir, git := initTestRepo(t)
	writeFile(t, dir, "app.go", "package app\nfunc A() int { return 1 }\n")
	git("add", ".")
	git("commit", "-m", "base")
	base := git("rev-parse", "HEAD")

	writeFile(t, dir, "app.go", "package app\nfunc A() int { return 2 }\n")
	git("add", ".")
	git("commit", "-m", "head")
	head := git("rev-parse", "HEAD")

	// Modify a tracked file without committing: CollectRangeState must refuse.
	writeFile(t, dir, "app.go", "package app\nfunc A() int { return 999 }\n")
	_, err := CollectRangeState(context.Background(), dir, base, head)
	if !errors.Is(err, ErrDirtyTrackedWorktree) {
		t.Fatalf("dirty tracked worktree err = %v, want ErrDirtyTrackedWorktree", err)
	}

	res := RunRange(context.Background(), RangeInput{
		GateMode: "enforce",
		BaseSHA:  base,
		HeadSHA:  head,
		GitErr:   err,
		Rigor:    fakeDCVerify(t, cleanDCVerifyJSON),
	})
	if res.Status != "error" || res.Passed || !res.RigorError || res.RigorErrorCode != RangeErrDirtyWorktree {
		t.Fatalf("dirty worktree RunRange = status=%q passed=%t err=%t code=%q", res.Status, res.Passed, res.RigorError, res.RigorErrorCode)
	}
}

func TestRunRangePassesBinaryPathsAsPlannedForNulPrefixedSecret(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("shell script fake requires unix")
	}
	dir, git := initTestRepo(t)
	writeFile(t, dir, "README.md", "hi\n")
	git("add", ".")
	git("commit", "-m", "base")
	base := git("rev-parse", "HEAD")

	// Commit a NUL-prefixed file containing a credential so git treats it as binary.
	writeFile(t, dir, "config.env", "\x00AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE\n")
	git("add", ".")
	git("commit", "-m", "add binary secret")
	head := git("rev-parse", "HEAD")

	state, err := CollectRangeState(context.Background(), dir, base, head)
	if err != nil {
		t.Fatalf("CollectRangeState: %v", err)
	}
	if len(state.BinaryPaths) != 1 || state.BinaryPaths[0] != "config.env" {
		t.Fatalf("BinaryPaths = %v, want [config.env]", state.BinaryPaths)
	}

	// Fake dcverify only reports the secret if --planned includes config.env
	// (matching rust/dc-verify/src/bin/dcverify.rs binary post-image scope rule).
	bin := filepath.Join(t.TempDir(), "fake-dcverify-binscan")
	script := `#!/bin/sh
cat >/dev/null
saw_planned=0
for arg in "$@"; do
  case "$arg" in
    *config.env*) saw_planned=1 ;;
  esac
done
if [ "$saw_planned" -eq 1 ]; then
  printf '%s\n' '{"ok":true,"files":1,"in_scope":["config.env"],"orphans":[],"untouched_planned":[],"findings":[{"gate":"secret_scan","severity":"blocking","strength":"derived","path":"config.env","line":1,"evidence":"AWS_ACCESS_KEY_ID=[REDACTED]","message":"credential detected"}],"coverage_unmeasured":[],"coverage_gaps":[],"coverage_skipped_by_type":["config.env"],"stub_ast_parsed_files":0,"runner_hijack_checked":true,"lean_tcb_checked":false,"stub_ast_degraded":[],"substance":{"added_lines":0,"substantive_lines":0,"trivial":0,"moved":0,"repeated":0,"generated":0,"judged":false,"low":false,"files":[]}}'
else
  printf '%s\n' '{"ok":true,"files":1,"in_scope":[],"orphans":["config.env"],"untouched_planned":[],"findings":[],"coverage_unmeasured":[],"coverage_gaps":[],"coverage_skipped_by_type":["config.env"],"stub_ast_parsed_files":0,"runner_hijack_checked":true,"lean_tcb_checked":false,"stub_ast_degraded":[],"substance":{"added_lines":0,"substantive_lines":0,"trivial":0,"moved":0,"repeated":0,"generated":0,"judged":false,"low":false,"files":[]}}'
fi
`
	if err := os.WriteFile(bin, []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	client := dcverify.New(bin, dir)
	client.Timeout = 5 * time.Second

	res := RunRange(context.Background(), RangeInput{
		Root:     dir,
		GateMode: "enforce",
		BaseSHA:  base,
		HeadSHA:  head,
		State:    state,
		Rigor:    client,
	})
	if res.Status != "blocked" || res.Passed || res.BlockingGapCount != 1 {
		t.Fatalf("NUL-prefixed secret file was not blocked: status=%q passed=%t blocking=%d", res.Status, res.Passed, res.BlockingGapCount)
	}
}

func TestRunRangePassAndAllowStubCounting(t *testing.T) {
	reply := `{"ok":true,"files":1,"in_scope":[],"orphans":["app.go"],"untouched_planned":[],"findings":[{"gate":"stub_allowed","severity":"advisory","strength":"derived","path":"app.go","line":4,"evidence":"// allow-stub: tracked in #1","message":"declared stub"}],"coverage_unmeasured":["app.go"],"coverage_gaps":[],"coverage_skipped_by_type":[],"stub_ast_parsed_files":1,"runner_hijack_checked":true,"lean_tcb_checked":false,"stub_ast_degraded":[],"substance":{"added_lines":2,"substantive_lines":2,"trivial":0,"moved":0,"repeated":0,"generated":0,"judged":false,"low":false,"files":[{"path":"app.go","added_lines":2,"substantive_lines":2}]}}`
	res := RunRange(context.Background(), RangeInput{
		GateMode: "enforce",
		BaseSHA:  strings.Repeat("a", 40),
		HeadSHA:  strings.Repeat("b", 40),
		State: RangeState{
			Changed: []string{"app.go"},
			Diff:    "diff --git a/app.go b/app.go\n--- a/app.go\n+++ b/app.go\n@@ -1 +1 @@\n+// allow-stub: tracked\n",
		},
		Rigor: fakeDCVerify(t, reply),
	})
	if res.Status != "verified" || !res.Passed {
		t.Fatalf("status=%q passed=%t degraded=%+v", res.Status, res.Passed, res.Degraded)
	}
	if res.StubAllowedCount != 1 || len(res.StubAllowed) != 1 {
		t.Fatalf("StubAllowedCount=%d len=%d, want 1", res.StubAllowedCount, len(res.StubAllowed))
	}
}

func TestRunRangeFailClosedOnMissingOrBrokenDCVerify(t *testing.T) {
	base := strings.Repeat("a", 40)
	head := strings.Repeat("b", 40)
	state := RangeState{
		Changed: []string{"app.go"},
		Diff:    "diff --git a/app.go b/app.go\n--- a/app.go\n+++ b/app.go\n@@ -1 +1 @@\n+x := 1\n",
	}

	// Missing dcverify -> error (no AllowNoRigor in pr-gate).
	res := RunRange(context.Background(), RangeInput{
		GateMode: "enforce",
		BaseSHA:  base,
		HeadSHA:  head,
		State:    state,
		Rigor:    nil,
	})
	if res.Status != "error" || !res.RigorError || res.RigorErrorCode != RangeErrDCVerifyMissing || res.Passed {
		t.Fatalf("missing dcverify: got status=%q err=%t code=%q passed=%t", res.Status, res.RigorError, res.RigorErrorCode, res.Passed)
	}

	// Broken dcverify -> error.
	broken := dcverify.New(filepath.Join(t.TempDir(), "does-not-exist"), t.TempDir())
	res2 := RunRange(context.Background(), RangeInput{
		GateMode: "enforce",
		BaseSHA:  base,
		HeadSHA:  head,
		State:    state,
		Rigor:    broken,
	})
	if res2.Status != "error" || !res2.RigorError || res2.RigorErrorCode != RangeErrDCVerifyFailed || res2.Passed {
		t.Fatalf("broken dcverify: got status=%q err=%t code=%q passed=%t", res2.Status, res2.RigorError, res2.RigorErrorCode, res2.Passed)
	}

	// Git error -> error.
	res3 := RunRange(context.Background(), RangeInput{
		GateMode: "enforce",
		BaseSHA:  base,
		HeadSHA:  head,
		GitErr:   errors.New("bad object"),
		Rigor:    fakeDCVerify(t, cleanDCVerifyJSON),
	})
	if res3.Status != "error" || !res3.RigorError || res3.RigorErrorCode != RangeErrGitError {
		t.Fatalf("git error: got status=%q err=%t code=%q", res3.Status, res3.RigorError, res3.RigorErrorCode)
	}
}

func TestRunRangeAdvisoryStillBlocksHardSafetyGaps(t *testing.T) {
	secretReply := `{"ok":true,"files":1,"in_scope":[],"orphans":["app.go"],"untouched_planned":[],"findings":[{"gate":"secret_scan","severity":"blocking","strength":"derived","path":"app.go","line":2,"evidence":"token = [REDACTED]","message":"credential detected"}],"coverage_unmeasured":["app.go"],"coverage_gaps":[],"coverage_skipped_by_type":[],"stub_ast_parsed_files":1,"runner_hijack_checked":true,"lean_tcb_checked":false,"stub_ast_degraded":[],"substance":{"added_lines":1,"substantive_lines":1,"trivial":0,"moved":0,"repeated":0,"generated":0,"judged":false,"low":false,"files":[{"path":"app.go","added_lines":1,"substantive_lines":1}]}}`
	res := RunRange(context.Background(), RangeInput{
		GateMode: "advisory",
		BaseSHA:  strings.Repeat("a", 40),
		HeadSHA:  strings.Repeat("b", 40),
		State: RangeState{
			Changed: []string{"app.go"},
			Diff:    "diff --git a/app.go b/app.go\n--- a/app.go\n+++ b/app.go\n@@ -1 +1 @@\n+x\n",
		},
		Rigor: fakeDCVerify(t, secretReply),
	})
	if res.Status != "blocked" || res.Passed || res.BlockingGapCount != 1 || res.HardSafetyCount != 1 {
		t.Fatalf("advisory secret finding: status=%q passed=%t blocking=%d hard=%d", res.Status, res.Passed, res.BlockingGapCount, res.HardSafetyCount)
	}
}

func TestRunRangeModeOffSkipsEverything(t *testing.T) {
	res := RunRange(context.Background(), RangeInput{
		GateMode: "off",
		BaseSHA:  strings.Repeat("a", 40),
		HeadSHA:  strings.Repeat("b", 40),
		Rigor:    nil,
	})
	if res.Status != "skipped" || res.Passed || res.RigorError {
		t.Fatalf("mode off: status=%q passed=%t err=%t", res.Status, res.Passed, res.RigorError)
	}
}

func TestRunRangeDegradedCases(t *testing.T) {
	base := strings.Repeat("a", 40)
	head := strings.Repeat("b", 40)
	diff := "diff --git a/app.go b/app.go\n--- a/app.go\n+++ b/app.go\n@@ -1 +1 @@\n+x\n"

	// 1. Diff too large -> degraded, never partial scan.
	res := RunRange(context.Background(), RangeInput{
		GateMode: "enforce",
		BaseSHA:  base,
		HeadSHA:  head,
		State:    RangeState{Changed: []string{"app.go"}, DiffTooLarge: true},
		Rigor:    fakeDCVerify(t, cleanDCVerifyJSON),
	})
	if res.Status != "degraded" || res.Passed || len(res.Degraded) != 1 || res.Degraded[0].Code != DegradedDiffTooLarge {
		t.Fatalf("diff too large: status=%q passed=%t degraded=%+v", res.Status, res.Passed, res.Degraded)
	}

	// 2. Moved gitlink -> degraded.
	res = RunRange(context.Background(), RangeInput{
		GateMode: "enforce",
		BaseSHA:  base,
		HeadSHA:  head,
		State: RangeState{
			Changed:       []string{"app.go", "sub"},
			Diff:          diff,
			MovedGitlinks: []proc.GitlinkChange{{Path: "sub", Shape: proc.GitlinkMoved}},
		},
		Rigor: fakeDCVerify(t, cleanDCVerifyJSON),
	})
	if res.Status != "degraded" || res.Passed || len(res.Degraded) != 1 || res.Degraded[0].Code != DegradedGitlinkMoved {
		t.Fatalf("moved gitlink: status=%q passed=%t degraded=%+v", res.Status, res.Passed, res.Degraded)
	}

	// 3. StubASTDegraded + binary scan skipped -> degraded.
	degradedReply := `{"ok":true,"files":2,"in_scope":[],"orphans":["app.go","blob.bin"],"untouched_planned":[],"findings":[],"coverage_unmeasured":["app.go"],"coverage_gaps":[],"coverage_skipped_by_type":["blob.bin"],"stub_ast_parsed_files":0,"runner_hijack_checked":true,"lean_tcb_checked":false,"stub_ast_degraded":["app.go: parse error","blob.bin: secret scan skipped (binary file not read: too large)"],"substance":{"added_lines":1,"substantive_lines":1,"trivial":0,"moved":0,"repeated":0,"generated":0,"judged":false,"low":false,"files":[{"path":"app.go","added_lines":1,"substantive_lines":1}]}}`
	res = RunRange(context.Background(), RangeInput{
		GateMode: "enforce",
		BaseSHA:  base,
		HeadSHA:  head,
		State:    RangeState{Changed: []string{"app.go", "blob.bin"}, Diff: diff},
		Rigor:    fakeDCVerify(t, degradedReply),
	})
	if res.Status != "degraded" || res.Passed || len(res.Degraded) != 2 {
		t.Fatalf("stub/binary degraded: status=%q passed=%t degraded=%+v", res.Status, res.Passed, res.Degraded)
	}

	// 4. Language with no stub AST checks (stub_ast_parsed_files == 0, no stub_ast_degraded): NOT degraded!
	noASTReply := `{"ok":true,"files":1,"in_scope":[],"orphans":["README.md"],"untouched_planned":[],"findings":[],"coverage_unmeasured":[],"coverage_gaps":[],"coverage_skipped_by_type":["README.md"],"stub_ast_parsed_files":0,"runner_hijack_checked":true,"lean_tcb_checked":false,"stub_ast_degraded":[],"substance":{"added_lines":1,"substantive_lines":1,"trivial":0,"moved":0,"repeated":0,"generated":0,"judged":false,"low":false,"files":[{"path":"README.md","added_lines":1,"substantive_lines":1}]}}`
	res = RunRange(context.Background(), RangeInput{
		GateMode: "enforce",
		BaseSHA:  base,
		HeadSHA:  head,
		State:    RangeState{Changed: []string{"README.md"}, Diff: diff},
		Rigor:    fakeDCVerify(t, noASTReply),
	})
	if res.Status != "verified" || !res.Passed || !res.StubASTNotApplicable {
		t.Fatalf("non-AST language: status=%q passed=%t na=%t", res.Status, res.Passed, res.StubASTNotApplicable)
	}
}
