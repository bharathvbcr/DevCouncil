package main

import (
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/prgate"
)

func setupPRGateRepo(t *testing.T) (string, func(args ...string) string) {
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
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
		return strings.TrimSpace(string(out))
	}
	runGit("init", "-b", "main")
	return dir, runGit
}

func writeRepoFile(t *testing.T, root, rel, content string) {
	t.Helper()
	full := filepath.Join(root, filepath.FromSlash(rel))
	if err := os.MkdirAll(filepath.Dir(full), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(full, []byte(content), 0o644); err != nil {
		t.Fatal(err)
	}
}

func makeFakeDevmapForPRGate(t *testing.T, baseSHA, headSHA string) string {
	t.Helper()
	if runtime.GOOS == "windows" {
		t.Skip("shell script fakes require unix")
	}
	binDir := t.TempDir()
	bin := filepath.Join(binDir, "devmap")
	blastJSON := `{"change":"` + baseSHA + `..` + headSHA + `","complete":true,"changed_files":[{"path":"app.go","status":{"kind":"modified"}}],"seeds":[{"file_path":"app.go","qualified_name":"app.go::Run","start_line":1,"end_line":3}],"impacted":[],"tests":[{"distance":1,"path":"app_test.go"}],"unattributed":[],"unavailable":[]}`
	script := `#!/bin/sh
if [ "$1" = "--json" ]; then
  shift
fi
case "$1" in
  manifest)
    echo "Usage: devmap manifest --graph-output PATH"
    exit 0
    ;;
  build)
    echo '{"nodes":2,"edges":1}'
    exit 0
    ;;
  status)
    echo '{"db_path":".devcouncil/codeintel/devmap.sqlite","generation_id":1,"node_count":2,"edge_count":1,"pending_count":0,"quarantined_count":0,"is_fresh":true,"schema_version":29,"expected_schema_version":29,"reader_ready":true,"query_ready":true}'
    exit 0
    ;;
  blast)
    printf '%s\n' '` + blastJSON + `'
    exit 0
    ;;
  *)
    echo "unexpected devmap args: $*" >&2
    exit 1
    ;;
esac
`
	if err := os.WriteFile(bin, []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	return bin
}

func makeFakeDCVerifyForPRGate(t *testing.T, jsonReply string) string {
	t.Helper()
	if runtime.GOOS == "windows" {
		t.Skip("shell script fakes require unix")
	}
	binDir := t.TempDir()
	bin := filepath.Join(binDir, "dcverify")
	script := "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '" + jsonReply + "'\n"
	if err := os.WriteFile(bin, []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	return bin
}

const (
	e2eCleanDCVerify  = `{"ok":true,"files":1,"in_scope":[],"orphans":["app.go"],"untouched_planned":[],"findings":[],"coverage_unmeasured":["app.go"],"coverage_gaps":[],"coverage_skipped_by_type":[],"stub_ast_parsed_files":1,"runner_hijack_checked":true,"lean_tcb_checked":false,"stub_ast_degraded":[],"substance":{"added_lines":1,"substantive_lines":1,"trivial":0,"moved":0,"repeated":0,"generated":0,"judged":false,"low":false,"files":[{"path":"app.go","added_lines":1,"substantive_lines":1}]}}`
	e2eSecretDCVerify = `{"ok":true,"files":1,"in_scope":[],"orphans":["app.go"],"untouched_planned":[],"findings":[{"gate":"secret_scan","severity":"blocking","strength":"derived","path":"app.go","line":2,"evidence":"key = [REDACTED]","message":"credential detected"},{"gate":"stub_detection","severity":"blocking","strength":"derived","path":"app.go","line":3,"evidence":"todo!()","message":"stub detected"}],"coverage_unmeasured":["app.go"],"coverage_gaps":[],"coverage_skipped_by_type":[],"stub_ast_parsed_files":1,"runner_hijack_checked":true,"lean_tcb_checked":false,"stub_ast_degraded":[],"substance":{"added_lines":2,"substantive_lines":2,"trivial":0,"moved":0,"repeated":0,"generated":0,"judged":false,"low":false,"files":[{"path":"app.go","added_lines":2,"substantive_lines":2}]}}`
)

func TestPRGateE2EPassAndOutputs(t *testing.T) {
	repo, git := setupPRGateRepo(t)
	writeRepoFile(t, repo, "app.go", "package app\nfunc Run() int { return 1 }\n")
	git("add", ".")
	git("commit", "-m", "base")
	base := git("rev-parse", "HEAD")

	writeRepoFile(t, repo, "app.go", "package app\nfunc Run() int { return 2 }\n")
	git("add", ".")
	git("commit", "-m", "head")
	head := git("rev-parse", "HEAD")

	devmapBin := makeFakeDevmapForPRGate(t, base, head)
	dcverifyBin := makeFakeDCVerifyForPRGate(t, e2eCleanDCVerify)
	outDir := t.TempDir()
	jsonPath := filepath.Join(outDir, "pr-gate.json")
	mdPath := filepath.Join(outDir, "pr-gate.md")
	sarifPath := filepath.Join(outDir, "pr-gate.sarif")

	stdout, restoreOut := swapStdout(t)
	stderr, restoreErr := swapStderr(t)
	code := dispatch([]string{
		"pr-gate",
		"--repo", repo,
		"--base", base,
		"--head", head,
		"--mode", "enforce",
		"--devmap", devmapBin,
		"--dcverify", dcverifyBin,
		"--format", "json",
		"--json-out", jsonPath,
		"--markdown-out", mdPath,
		"--sarif-out", sarifPath,
	})
	restoreErr()
	restoreOut()

	if code != prgate.ExitPass {
		t.Fatalf("exit=%d want 0\nstderr=%s\nstdout=%s", code, stderr.String(), stdout.String())
	}

	var rep prgate.Report
	if err := json.Unmarshal(stdout.Bytes(), &rep); err != nil {
		t.Fatalf("stdout json: %v\n%s", err, stdout.String())
	}
	if rep.Schema != prgate.SchemaVersion || rep.Outcome != prgate.OutcomeSuccess || rep.StatusContext != prgate.StatusContext || rep.StatusState != "success" {
		t.Fatalf("report = %+v", rep)
	}
	mdBytes, err := os.ReadFile(mdPath)
	if err != nil {
		t.Fatalf("read markdown out: %v", err)
	}
	if !strings.HasPrefix(string(mdBytes), prgate.StickyMarker+"\n") {
		t.Fatalf("markdown missing sticky marker:\n%s", mdBytes)
	}
	if strings.Contains(string(mdBytes), "app.go::Run") {
		t.Fatalf("markdown leaked symbol name:\n%s", mdBytes)
	}
	sarifBytes, err := os.ReadFile(sarifPath)
	if err != nil {
		t.Fatalf("read sarif out: %v", err)
	}
	if !strings.Contains(string(sarifBytes), `"version": "2.1.0"`) {
		t.Fatalf("unexpected sarif:\n%s", sarifBytes)
	}
}

func TestPRGateE2EBlockingSecretAndStubFail(t *testing.T) {
	repo, git := setupPRGateRepo(t)
	writeRepoFile(t, repo, "app.go", "package app\nfunc Run() int { return 1 }\n")
	git("add", ".")
	git("commit", "-m", "base")
	base := git("rev-parse", "HEAD")

	writeRepoFile(t, repo, "app.go", "package app\nvar k = \"secret\"\nfunc Run() int { panic(\"todo\") }\n")
	git("add", ".")
	git("commit", "-m", "head")
	head := git("rev-parse", "HEAD")

	devmapBin := makeFakeDevmapForPRGate(t, base, head)
	dcverifyBin := makeFakeDCVerifyForPRGate(t, e2eSecretDCVerify)

	stdout, restoreOut := swapStdout(t)
	_, restoreErr := swapStderr(t)
	code := dispatch([]string{
		"pr-gate",
		"--repo", repo,
		"--base", base,
		"--head", head,
		"--devmap", devmapBin,
		"--dcverify", dcverifyBin,
		"--format", "json",
	})
	restoreErr()
	restoreOut()

	if code != prgate.ExitFail {
		t.Fatalf("exit=%d want 1\nstdout=%s", code, stdout.String())
	}
	var rep prgate.Report
	if err := json.Unmarshal(stdout.Bytes(), &rep); err != nil {
		t.Fatal(err)
	}
	if rep.Outcome != prgate.OutcomeFailure || rep.StatusState != "failure" || rep.Summary.BlockingFindings != 2 {
		t.Fatalf("report = %+v", rep)
	}
}

func TestPRGateE2EDegradedGitattributesDiffSuppression(t *testing.T) {
	repo, git := setupPRGateRepo(t)
	writeRepoFile(t, repo, ".gitattributes", "*.go -diff\n")
	writeRepoFile(t, repo, "app.go", "package app\nfunc Run() int { return 1 }\n")
	git("add", ".")
	git("commit", "-m", "base")
	base := git("rev-parse", "HEAD")

	writeRepoFile(t, repo, "app.go", "package app\nfunc Run() int { return 2 }\n")
	git("add", ".")
	git("commit", "-m", "head")
	head := git("rev-parse", "HEAD")

	devmapBin := makeFakeDevmapForPRGate(t, base, head)
	dcverifyBin := makeFakeDCVerifyForPRGate(t, e2eCleanDCVerify)

	stdout, restoreOut := swapStdout(t)
	_, restoreErr := swapStderr(t)
	code := dispatch([]string{
		"pr-gate",
		"--repo", repo,
		"--base", base,
		"--head", head,
		"--devmap", devmapBin,
		"--dcverify", dcverifyBin,
		"--format", "json",
	})
	restoreErr()
	restoreOut()

	if code != prgate.ExitError {
		t.Fatalf("exit=%d want 2 (degraded)\nstdout=%s", code, stdout.String())
	}
	var rep prgate.Report
	if err := json.Unmarshal(stdout.Bytes(), &rep); err != nil {
		t.Fatal(err)
	}
	if rep.Outcome != prgate.OutcomeError || rep.StatusState != "error" || rep.Summary.Degraded == 0 {
		t.Fatalf("expected degraded error report, got %+v", rep)
	}
}

func TestPRGateE2EHeadMismatchError(t *testing.T) {
	repo, git := setupPRGateRepo(t)
	writeRepoFile(t, repo, "app.go", "package app\nfunc Run() int { return 1 }\n")
	git("add", ".")
	git("commit", "-m", "base")
	base := git("rev-parse", "HEAD")

	writeRepoFile(t, repo, "app.go", "package app\nfunc Run() int { return 2 }\n")
	git("add", ".")
	git("commit", "-m", "head")
	head := git("rev-parse", "HEAD")

	// 1. Checkout HEAD != --head -> exit 2 (checkout_head_mismatch).
	wrongHead := strings.Repeat("9", 40)
	stdout, restoreOut := swapStdout(t)
	_, restoreErr := swapStderr(t)
	code := dispatch([]string{
		"pr-gate",
		"--repo", repo,
		"--base", base,
		"--head", wrongHead,
		"--format", "json",
	})
	restoreErr()
	restoreOut()
	if code != prgate.ExitError {
		t.Fatalf("checkout mismatch exit=%d want 2\nstdout=%s", code, stdout.String())
	}
	var rep prgate.Report
	if err := json.Unmarshal(stdout.Bytes(), &rep); err != nil {
		t.Fatal(err)
	}
	if rep.ErrorCode != "checkout_head_mismatch" || rep.StatusState != "error" {
		t.Fatalf("report = %+v", rep)
	}

	// 2. Checkout matches --head, but devmap blast reports a different Head() -> exit 2 (blast_head_mismatch).
	devmapBin := makeFakeDevmapForPRGate(t, base, wrongHead)
	dcverifyBin := makeFakeDCVerifyForPRGate(t, e2eCleanDCVerify)
	stdout2, restoreOut2 := swapStdout(t)
	_, restoreErr2 := swapStderr(t)
	code2 := dispatch([]string{
		"pr-gate",
		"--repo", repo,
		"--base", base,
		"--head", head,
		"--devmap", devmapBin,
		"--dcverify", dcverifyBin,
		"--format", "json",
	})
	restoreErr2()
	restoreOut2()
	if code2 != prgate.ExitError {
		t.Fatalf("blast head mismatch exit=%d want 2\nstdout=%s", code2, stdout2.String())
	}
	var rep2 prgate.Report
	if err := json.Unmarshal(stdout2.Bytes(), &rep2); err != nil {
		t.Fatal(err)
	}
	if rep2.ErrorCode != prgate.BlastErrHeadMismatch || rep2.StatusState != "error" {
		t.Fatalf("report2 = %+v", rep2)
	}
}

func TestPRGateE2EModeOffAndUsageError(t *testing.T) {
	repo := t.TempDir()

	// Mode off -> skipped, ShouldPost=false, exit 0.
	stdout, restoreOut := swapStdout(t)
	_, restoreErr := swapStderr(t)
	code := dispatch([]string{
		"pr-gate",
		"--repo", repo,
		"--mode", "off",
		"--format", "json",
	})
	restoreErr()
	restoreOut()
	if code != prgate.ExitPass {
		t.Fatalf("mode off exit=%d want 0", code)
	}
	var rep prgate.Report
	if err := json.Unmarshal(stdout.Bytes(), &rep); err != nil {
		t.Fatal(err)
	}
	if rep.Outcome != prgate.OutcomeSkipped || rep.ShouldPost || rep.StatusState != "" {
		t.Fatalf("mode off report = %+v", rep)
	}

	// Usage error (non-40-hex --base) -> exit 64.
	_, restoreErr2 := swapStderr(t)
	code2 := dispatch([]string{
		"pr-gate",
		"--repo", repo,
		"--base", "not-a-sha",
	})
	restoreErr2()
	if code2 != prgate.ExitUsage {
		t.Fatalf("bad --base exit=%d want 64", code2)
	}
}

func TestPRGateE2ENulPrefixedBinarySecretAndDirtyWorktree(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("shell script fakes require unix")
	}
	repo, git := setupPRGateRepo(t)
	writeRepoFile(t, repo, "app.go", "package app\nfunc Run() int { return 1 }\n")
	git("add", ".")
	git("commit", "-m", "base")
	base := git("rev-parse", "HEAD")

	// Commit a NUL-prefixed secret file so git treats it as binary.
	writeRepoFile(t, repo, "config.env", "\x00AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE\n")
	git("add", ".")
	git("commit", "-m", "add nul-prefixed secret")
	head := git("rev-parse", "HEAD")

	// Fake devmap reporting config.env as changed.
	dmDir := t.TempDir()
	devmapBin := filepath.Join(dmDir, "devmap")
	blastJSON := `{"change":"` + base + `..` + head + `","complete":true,"changed_files":[{"path":"config.env","status":{"kind":"added"}}],"seeds":[],"impacted":[],"tests":[],"unattributed":[],"unavailable":[]}`
	dmScript := `#!/bin/sh
if [ "$1" = "--json" ]; then shift; fi
case "$1" in
  manifest) echo "Usage: devmap manifest --graph-output PATH"; exit 0 ;;
  build) echo '{"nodes":1,"edges":0}'; exit 0 ;;
  status) echo '{"db_path":".devcouncil/codeintel/devmap.sqlite","generation_id":1,"node_count":1,"edge_count":0,"pending_count":0,"quarantined_count":0,"is_fresh":true,"schema_version":29,"expected_schema_version":29,"reader_ready":true,"query_ready":true}'; exit 0 ;;
  blast) printf '%s\n' '` + blastJSON + `'; exit 0 ;;
  *) exit 1 ;;
esac
`
	if err := os.WriteFile(devmapBin, []byte(dmScript), 0o755); err != nil {
		t.Fatal(err)
	}

	// Fake dcverify only catches the binary secret if --planned carries config.env.
	dcvDir := t.TempDir()
	dcverifyBin := filepath.Join(dcvDir, "dcverify")
	dcvScript := `#!/bin/sh
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
	if err := os.WriteFile(dcverifyBin, []byte(dcvScript), 0o755); err != nil {
		t.Fatal(err)
	}

	stdout, restoreOut := swapStdout(t)
	_, restoreErr := swapStderr(t)
	code := dispatch([]string{
		"pr-gate",
		"--repo", repo,
		"--base", base,
		"--head", head,
		"--devmap", devmapBin,
		"--dcverify", dcverifyBin,
		"--format", "json",
	})
	restoreErr()
	restoreOut()

	if code != prgate.ExitFail {
		t.Fatalf("NUL-prefixed binary secret exit=%d want 1\nstdout=%s", code, stdout.String())
	}
	var rep prgate.Report
	if err := json.Unmarshal(stdout.Bytes(), &rep); err != nil {
		t.Fatal(err)
	}
	if rep.Outcome != prgate.OutcomeFailure || rep.Summary.BlockingFindings != 1 {
		t.Fatalf("expected failure for NUL-prefixed binary secret, got %+v", rep)
	}

	// Now dirty a tracked file in the working tree: pr-gate must fail closed with error (exit 2).
	writeRepoFile(t, repo, "app.go", "package app\nfunc Run() int { return 999 }\n")
	stdout2, restoreOut2 := swapStdout(t)
	_, restoreErr2 := swapStderr(t)
	code2 := dispatch([]string{
		"pr-gate",
		"--repo", repo,
		"--base", base,
		"--head", head,
		"--devmap", devmapBin,
		"--dcverify", dcverifyBin,
		"--format", "json",
	})
	restoreErr2()
	restoreOut2()

	if code2 != prgate.ExitError {
		t.Fatalf("dirty tracked worktree exit=%d want 2\nstdout=%s", code2, stdout2.String())
	}
	var rep2 prgate.Report
	if err := json.Unmarshal(stdout2.Bytes(), &rep2); err != nil {
		t.Fatal(err)
	}
	if rep2.Outcome != prgate.OutcomeError || rep2.ErrorCode != "dirty_worktree" {
		t.Fatalf("dirty tracked worktree report = %+v", rep2)
	}
}

func findRealBinForE2E(t *testing.T, name string) string {
	t.Helper()
	candidate := filepath.Join("..", "..", "..", "..", "rust", "target", "debug", name)
	if runtime.GOOS == "windows" {
		candidate += ".exe"
	}
	if abs, err := filepath.Abs(candidate); err == nil {
		if info, err := os.Stat(abs); err == nil && !info.IsDir() {
			return abs
		}
	}
	if p, err := exec.LookPath(name); err == nil {
		return p
	}
	return ""
}

func TestPRGateE2EEgressScrubbingAndDCRedactFailClosed(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("shell script fakes require unix")
	}
	awsKey := "AKIA" + "IOSFODNN7EXAMPLE"
	ghpTok := "ghp_" + strings.Repeat("A1b2C3d4", 4) + "abcd"
	plantedPath := "pkg/" + awsKey + "_" + ghpTok + ".go"

	repo, git := setupPRGateRepo(t)
	writeRepoFile(t, repo, "app.go", "package app\nfunc Run() int { return 1 }\n")
	git("add", ".")
	git("commit", "-m", "base")
	base := git("rev-parse", "HEAD")

	writeRepoFile(t, repo, plantedPath, "package pkg\nfunc Stub() { panic(\"todo\") }\n")
	git("add", ".")
	git("commit", "-m", "add planted path")
	head := git("rev-parse", "HEAD")

	dmDir := t.TempDir()
	devmapBin := filepath.Join(dmDir, "devmap")
	blastJSON := `{"change":"` + base + `..` + head + `","complete":true,"changed_files":[{"path":"` + plantedPath + `","status":{"kind":"added"}}],"seeds":[],"impacted":[],"tests":[],"unattributed":[],"unavailable":[]}`
	dmScript := `#!/bin/sh
if [ "$1" = "--json" ]; then shift; fi
case "$1" in
  manifest) echo "Usage: devmap manifest --graph-output PATH"; exit 0 ;;
  build) echo '{"nodes":1,"edges":0}'; exit 0 ;;
  status) echo '{"db_path":".devcouncil/codeintel/devmap.sqlite","generation_id":1,"node_count":1,"edge_count":0,"pending_count":0,"quarantined_count":0,"is_fresh":true,"schema_version":29,"expected_schema_version":29,"reader_ready":true,"query_ready":true}'; exit 0 ;;
  blast) printf '%s\n' '` + blastJSON + `'; exit 0 ;;
  *) exit 1 ;;
esac
`
	if err := os.WriteFile(devmapBin, []byte(dmScript), 0o755); err != nil {
		t.Fatal(err)
	}

	dcverifyReply := `{"ok":true,"files":1,"in_scope":[],"orphans":["` + plantedPath + `"],"untouched_planned":[],"findings":[{"gate":"stub_detection","severity":"blocking","strength":"derived","path":"` + plantedPath + `","line":2,"evidence":"panic(\"todo\")","message":"stub detected"}],"coverage_unmeasured":["` + plantedPath + `"],"coverage_gaps":[],"coverage_skipped_by_type":[],"stub_ast_parsed_files":1,"runner_hijack_checked":true,"lean_tcb_checked":false,"stub_ast_degraded":[],"substance":{"added_lines":2,"substantive_lines":2,"trivial":0,"moved":0,"repeated":0,"generated":0,"judged":false,"low":false,"files":[{"path":"` + plantedPath + `","added_lines":2,"substantive_lines":2}]}}`
	dcverifyBin := makeFakeDCVerifyForPRGate(t, dcverifyReply)

	// 1. Pure-Go fallback scrubber (no --dcredact, unset DCREDACT_BIN).
	t.Setenv("DCREDACT_BIN", "")
	outDir := t.TempDir()
	jsonPath := filepath.Join(outDir, "pr-gate.json")
	mdPath := filepath.Join(outDir, "pr-gate.md")
	sarifPath := filepath.Join(outDir, "pr-gate.sarif")

	stdout, restoreOut := swapStdout(t)
	stderr, restoreErr := swapStderr(t)
	code := dispatch([]string{
		"pr-gate",
		"--repo", repo,
		"--base", base,
		"--head", head,
		"--devmap", devmapBin,
		"--dcverify", dcverifyBin,
		"--format", "annotations",
		"--json-out", jsonPath,
		"--markdown-out", mdPath,
		"--sarif-out", sarifPath,
	})
	restoreErr()
	restoreOut()

	if code != prgate.ExitFail {
		t.Fatalf("fallback scrub exit=%d want 1\nstderr=%s\nstdout=%s", code, stderr.String(), stdout.String())
	}
	for _, pair := range []struct {
		name string
		body string
	}{
		{"annotations", stdout.String()},
		{"json", mustReadFile(t, jsonPath)},
		{"markdown", mustReadFile(t, mdPath)},
		{"sarif", mustReadFile(t, sarifPath)},
	} {
		for _, secret := range []string{awsKey, ghpTok} {
			if strings.Contains(pair.body, secret) {
				t.Fatalf("%s leaked planted secret %q:\n%s", pair.name, secret, pair.body)
			}
		}
	}

	// 2. Real dcredact binary (if built) must also scrub all surfaces cleanly.
	if realDCRedact := findRealBinForE2E(t, "dcredact"); realDCRedact != "" {
		outDir2 := t.TempDir()
		mdPath2 := filepath.Join(outDir2, "pr-gate.md")
		sarifPath2 := filepath.Join(outDir2, "pr-gate.sarif")
		stdout2, restoreOut2 := swapStdout(t)
		stderr2, restoreErr2 := swapStderr(t)
		code2 := dispatch([]string{
			"pr-gate",
			"--repo", repo,
			"--base", base,
			"--head", head,
			"--devmap", devmapBin,
			"--dcverify", dcverifyBin,
			"--dcredact", realDCRedact,
			"--format", "annotations",
			"--markdown-out", mdPath2,
			"--sarif-out", sarifPath2,
		})
		restoreErr2()
		restoreOut2()
		if code2 != prgate.ExitFail {
			t.Fatalf("real dcredact exit=%d want 1\nstderr=%s\nstdout=%s", code2, stderr2.String(), stdout2.String())
		}
		for _, body := range []string{stdout2.String(), mustReadFile(t, mdPath2), mustReadFile(t, sarifPath2)} {
			if strings.Contains(body, awsKey) || strings.Contains(body, ghpTok) {
				t.Fatalf("real dcredact output leaked planted secret:\n%s", body)
			}
		}
	}

	// 3. Explicitly passed --dcredact that crashes MUST fail closed (exit 2)
	// and must not write output files.
	crashBin := filepath.Join(t.TempDir(), "dcredact")
	if err := os.WriteFile(crashBin, []byte("#!/bin/sh\necho crash >&2\nexit 1\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	failOutDir := t.TempDir()
	failMD := filepath.Join(failOutDir, "pr-gate.md")
	stdout3, restoreOut3 := swapStdout(t)
	_, restoreErr3 := swapStderr(t)
	code3 := dispatch([]string{
		"pr-gate",
		"--repo", repo,
		"--base", base,
		"--head", head,
		"--devmap", devmapBin,
		"--dcverify", dcverifyBin,
		"--dcredact", crashBin,
		"--format", "json",
		"--markdown-out", failMD,
	})
	restoreErr3()
	restoreOut3()
	if code3 != prgate.ExitError {
		t.Fatalf("crashing --dcredact exit=%d want 2\nstdout=%s", code3, stdout3.String())
	}
	if _, err := os.Stat(failMD); !os.IsNotExist(err) {
		t.Fatalf("crashing --dcredact must not write markdown output file")
	}

	// 4. --dcredact located inside repository under analysis must be refused (exit 2).
	insideBin := filepath.Join(repo, "evil-dcredact")
	if err := os.WriteFile(insideBin, []byte("#!/bin/sh\nexit 0\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	_, restoreOut4 := swapStdout(t)
	_, restoreErr4 := swapStderr(t)
	code4 := dispatch([]string{
		"pr-gate",
		"--repo", repo,
		"--base", base,
		"--head", head,
		"--devmap", devmapBin,
		"--dcverify", dcverifyBin,
		"--dcredact", insideBin,
		"--format", "json",
	})
	restoreErr4()
	restoreOut4()
	if code4 != prgate.ExitError {
		t.Fatalf("repo-local --dcredact exit=%d want 2", code4)
	}
}

func TestPRGateE2ECoverageRealGoProfile(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("shell script fakes require unix")
	}
	dcverifyBin := findRealBinForE2E(t, "dcverify")
	if dcverifyBin == "" {
		t.Skip("real dcverify binary not found for coverage E2E test")
	}

	repo, git := setupPRGateRepo(t)
	writeRepoFile(t, repo, "go.mod", "module example.com/covdemo\n\ngo 1.22\n")
	writeRepoFile(t, repo, "app.go", "package app\n\nfunc Run(x int) int {\n\treturn 1\n}\n")
	git("add", ".")
	git("commit", "-m", "base")
	base := git("rev-parse", "HEAD")

	// Replace line 4 inside Run with lines 4-7 so every added line is executable.
	writeRepoFile(t, repo, "app.go", "package app\n\nfunc Run(x int) int {\n\tif x > 0 {\n\t\treturn x + 1\n\t}\n\treturn 0\n}\n")
	git("add", ".")
	git("commit", "-m", "update Run")
	head := git("rev-parse", "HEAD")

	devmapBin := makeFakeDevmapForPRGate(t, base, head)
	covDir := t.TempDir()

	// 1. Omitted --coverage -> coverage.measured=false, status="not measured".
	stdoutNoCov, restoreOut0 := swapStdout(t)
	_, restoreErr0 := swapStderr(t)
	code0 := dispatch([]string{
		"pr-gate",
		"--repo", repo,
		"--base", base,
		"--head", head,
		"--devmap", devmapBin,
		"--dcverify", dcverifyBin,
		"--format", "json",
	})
	restoreErr0()
	restoreOut0()
	if code0 != prgate.ExitPass {
		t.Fatalf("no-coverage exit=%d want 0\nstdout=%s", code0, stdoutNoCov.String())
	}
	var rep0 prgate.Report
	if err := json.Unmarshal(stdoutNoCov.Bytes(), &rep0); err != nil {
		t.Fatal(err)
	}
	if rep0.Coverage.Measured || rep0.Coverage.Status != "not measured" {
		t.Fatalf("expected unmeasured coverage without --coverage, got %+v", rep0.Coverage)
	}

	// 2. Real Go coverprofile (mode: set) covering all added lines (lines 4-7, count=1).
	coveredProf := filepath.Join(covDir, "covered.out")
	coveredContent := "mode: set\n" +
		"example.com/covdemo/app.go:3.21,4.11 1 1\n" +
		"example.com/covdemo/app.go:4.11,6.3 1 1\n" +
		"example.com/covdemo/app.go:7.2,7.10 1 1\n"
	if err := os.WriteFile(coveredProf, []byte(coveredContent), 0o644); err != nil {
		t.Fatal(err)
	}
	mdCovPath := filepath.Join(covDir, "covered.md")
	stdoutCov, restoreOut1 := swapStdout(t)
	stderrCov, restoreErr1 := swapStderr(t)
	code1 := dispatch([]string{
		"pr-gate",
		"--repo", repo,
		"--base", base,
		"--head", head,
		"--devmap", devmapBin,
		"--dcverify", dcverifyBin,
		"--coverage", coveredProf,
		"--format", "json",
		"--markdown-out", mdCovPath,
	})
	restoreErr1()
	restoreOut1()
	if code1 != prgate.ExitPass {
		t.Fatalf("covered profile exit=%d want 0\nstderr=%s\nstdout=%s", code1, stderrCov.String(), stdoutCov.String())
	}
	var rep1 prgate.Report
	if err := json.Unmarshal(stdoutCov.Bytes(), &rep1); err != nil {
		t.Fatal(err)
	}
	if !rep1.Coverage.Measured || rep1.Coverage.Status != "measured" {
		t.Fatalf("expected measured coverage, got %+v", rep1.Coverage)
	}
	if rep1.Summary.AdvisoryFindings != 0 || rep1.Summary.BlockingFindings != 0 {
		t.Fatalf("expected 0 findings on fully covered diff, got findings=%+v", rep1.Findings)
	}
	if !strings.Contains(mustReadFile(t, mdCovPath), "- **Coverage:** measured\n") {
		t.Fatalf("markdown missing measured coverage label:\n%s", mustReadFile(t, mdCovPath))
	}

	// 3. Real Go coverprofile (mode: set) with UNCOVERED added lines (lines 4-7, count=0).
	uncoveredProf := filepath.Join(covDir, "uncovered.out")
	uncoveredContent := "mode: set\n" +
		"example.com/covdemo/app.go:3.21,4.11 1 0\n" +
		"example.com/covdemo/app.go:4.11,6.3 1 0\n" +
		"example.com/covdemo/app.go:7.2,7.10 1 0\n"
	if err := os.WriteFile(uncoveredProf, []byte(uncoveredContent), 0o644); err != nil {
		t.Fatal(err)
	}
	stdoutUncov, restoreOut2 := swapStdout(t)
	stderrUncov, restoreErr2 := swapStderr(t)
	code2 := dispatch([]string{
		"pr-gate",
		"--repo", repo,
		"--base", base,
		"--head", head,
		"--devmap", devmapBin,
		"--dcverify", dcverifyBin,
		"--coverage", uncoveredProf,
		"--format", "json",
	})
	restoreErr2()
	restoreOut2()
	if code2 != prgate.ExitPass {
		t.Fatalf("uncovered profile (advisory) exit=%d want 0\nstderr=%s\nstdout=%s", code2, stderrUncov.String(), stdoutUncov.String())
	}
	var rep2 prgate.Report
	if err := json.Unmarshal(stdoutUncov.Bytes(), &rep2); err != nil {
		t.Fatal(err)
	}
	if !rep2.Coverage.Measured || rep2.Coverage.Status != "measured" {
		t.Fatalf("expected measured coverage on uncovered diff, got %+v", rep2.Coverage)
	}
	foundDiffNotExercised := false
	for _, f := range rep2.Findings {
		if f.GapType == "diff_not_exercised" && !f.Blocking && f.Path == "app.go" {
			foundDiffNotExercised = true
		}
	}
	if !foundDiffNotExercised {
		t.Fatalf("expected advisory diff_not_exercised finding for app.go, got %+v", rep2.Findings)
	}

	// 4. Malformed coverage profile must fail closed (exit 2).
	badProf := filepath.Join(covDir, "bad.out")
	if err := os.WriteFile(badProf, []byte("not-a-coverage-profile\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	stdoutBad, restoreOut3 := swapStdout(t)
	_, restoreErr3 := swapStderr(t)
	code3 := dispatch([]string{
		"pr-gate",
		"--repo", repo,
		"--base", base,
		"--head", head,
		"--devmap", devmapBin,
		"--dcverify", dcverifyBin,
		"--coverage", badProf,
		"--format", "json",
	})
	restoreErr3()
	restoreOut3()
	if code3 != prgate.ExitError {
		t.Fatalf("malformed coverage profile exit=%d want 2\nstdout=%s", code3, stdoutBad.String())
	}
}

func mustReadFile(t *testing.T, path string) string {
	t.Helper()
	b, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read %s: %v", path, err)
	}
	return string(b)
}
