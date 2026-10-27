package verify_test

import (
	"bytes"
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

// gitDiffCap is the host cap (verify.gitDiffLimit). A file of this size is
// the largest a supplemental read will accept before the synthetic header.
const gitDiffCap = 32 << 20

func advTypes(gaps []verify.Gap) []string {
	out := make([]string, 0, len(gaps))
	for _, g := range gaps {
		out = append(out, g.GapType)
	}
	return out
}

func advRun(t *testing.T, root, taskID string, planned []dc.PlannedFile, changed []string, diff string, empty bool, gitErr error) (verify.MCPResult, []verify.Gap) {
	t.Helper()
	if gitErr != nil {
		changed, diff, empty = nil, "", true
	}
	in := verify.Input{
		Task: &store.Task{
			ID: taskID, Difficulty: "easy", PlannedFiles: planned,
		},
		GateMode:     "enforce",
		ChangedFiles: changed,
		DiffContent:  diff,
		DiffEmpty:    empty,
		WorkPresent:  !empty,
		GitErr:       gitErr,
		Rigor:        dcverify.New(testsupport.DCVerify(t), root),
	}
	gaps, meta := verify.Run(context.Background(), in)
	return verify.ToMCP(taskID, gaps, meta), gaps
}

func advNotVerified(t *testing.T, result verify.MCPResult) {
	t.Helper()
	if result.Passed || result.Status == "verified" {
		t.Fatalf("status=%s passed=%v", result.Status, result.Passed)
	}
}

func advNoSecret(t *testing.T, result verify.MCPResult, gaps []verify.Gap, extra ...string) {
	t.Helper()
	reportLeaks(t, result, gaps, exampleAWSKey())
	for _, s := range extra {
		blob := result.RigorSkippedReason + " " + strings.Join(result.DegradedReasons, " ")
		for _, g := range gaps {
			blob += " " + g.Description + " " + g.GapType + " " + strings.Join(g.Evidence, " ")
			if g.File != nil {
				blob += " " + *g.File
			}
		}
		if strings.Contains(blob, s) {
			t.Fatal("a gap or skip reason contains symlink target contents")
		}
	}
}

func advHas(gaps []verify.Gap, kind string) bool {
	for _, g := range gaps {
		if g.GapType == kind {
			return true
		}
	}
	return false
}

func writeBytes(t *testing.T, root, rel string, body []byte) {
	t.Helper()
	full := filepath.Join(root, filepath.FromSlash(rel))
	if err := os.MkdirAll(filepath.Dir(full), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(full, body, 0o644); err != nil {
		t.Fatal(err)
	}
}

// nulThenKey is an untracked file whose first byte is NUL and whose key sits
// past the 8192-byte binary probe, so a probe-only scan cannot see it.
func nulThenKey(t *testing.T) []byte {
	t.Helper()
	key := exampleAWSKey()
	line := []byte("\nlet aws_key = \"" + key + "\";\n")
	body := bytes.Repeat([]byte{'A'}, 9000)
	body[0] = 0
	return append(body, line...)
}

func TestAdvUntrackedNulThenKeyPlannedIsScanned(t *testing.T) {
	const rel = "src/new.rs"
	root := gitRepo(t)
	writeBytes(t, root, rel, nulThenKey(t))
	changed, diff, empty, err := verify.CollectGitState(context.Background(), root)
	if err != nil {
		t.Fatalf("readable untracked file was refused: %v", err)
	}
	result, gaps := advRun(t, root, "TASK-ADV-NUL", []dc.PlannedFile{
		{Path: rel, AllowedChange: dc.ChangeCreate},
	}, changed, diff, empty, nil)
	advNoSecret(t, result, gaps)
	advNotVerified(t, result)
	if !advHas(gaps, "security_risk") {
		t.Fatalf("planned untracked bytes with a leading NUL were not a secret finding (types=%v status=%s)", advTypes(gaps), result.Status)
	}
	if advHas(gaps, "rigor_check_unavailable") {
		t.Fatal("a readable planned file was rigor_check_unavailable instead of scanned")
	}
}

func TestAdvUntrackedNulThenKeyUnplannedIsNotVerified(t *testing.T) {
	root := gitRepo(t)
	writeBytes(t, root, "src/new.rs", nulThenKey(t))
	changed, diff, empty, err := verify.CollectGitState(context.Background(), root)
	if err != nil {
		result, gaps := advRun(t, root, "TASK-ADV-NUL-OUT", nil, nil, "", true, err)
		advNoSecret(t, result, gaps)
		advNotVerified(t, result)
		if !advHas(gaps, "rigor_check_unavailable") {
			t.Fatalf("refusal was not blocking-unavailable (types=%v status=%s)", advTypes(gaps), result.Status)
		}
		return
	}
	result, gaps := advRun(t, root, "TASK-ADV-NUL-OUT", []dc.PlannedFile{
		{Path: "src/other.rs", AllowedChange: dc.ChangeModify},
	}, changed, diff, empty, nil)
	advNoSecret(t, result, gaps)
	advNotVerified(t, result)
	if result.Status == "verified" || result.Passed {
		t.Fatal("unplanned unscanned bytes verified")
	}
	refused := advHas(gaps, "security_risk") || advHas(gaps, "rigor_check_unavailable") || advHas(gaps, "orphan_diff")
	if !refused {
		t.Fatalf("unplanned binary untracked file was neither scanned nor refused (types=%v status=%s)", advTypes(gaps), result.Status)
	}
	t.Logf("status=%s types=%v", result.Status, advTypes(gaps))
}

func TestAdvUntrackedKeyWithoutTodoIsASecret(t *testing.T) {
	const rel = "src/new.rs"
	key := exampleAWSKey()
	root := gitRepo(t)
	writeFile(t, root, rel, "let aws_key = \""+key+"\";\n")
	changed, diff, empty, err := verify.CollectGitState(context.Background(), root)
	if err != nil {
		t.Fatalf("readable text file was refused: %v", err)
	}
	result, gaps := advRun(t, root, "TASK-ADV-KEY", []dc.PlannedFile{
		{Path: rel, AllowedChange: dc.ChangeCreate},
	}, changed, diff, empty, nil)
	advNoSecret(t, result, gaps)
	advNotVerified(t, result)
	if !advHas(gaps, "security_risk") {
		t.Fatalf("planned untracked key was not a secret finding (types=%v status=%s)", advTypes(gaps), result.Status)
	}
	if advHas(gaps, "rigor_check_unavailable") {
		t.Fatal("readable planned text was rigor_check_unavailable")
	}
}

func TestAdvSymlinkBesideUntrackedFileFailsClosed(t *testing.T) {
	key := exampleAWSKey()
	const sentinel = "SYMLINK-TARGET-SENTINEL"
	root := gitRepo(t)
	outside := filepath.Join(t.TempDir(), "secret.txt")
	if err := os.WriteFile(outside, []byte(sentinel+"\n"+key+"\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	writeFile(t, root, "src/new.rs", "pub fn f() {\n    todo!()\n}\n")
	if err := os.Symlink(outside, filepath.Join(root, "link.rs")); err != nil {
		t.Fatal(err)
	}
	changed, diff, empty, err := verify.CollectGitState(context.Background(), root)
	if err == nil {
		t.Fatalf("symlink beside a real file was not fail-closed (empty=%v changed=%d diffLen=%d)", empty, len(changed), len(diff))
	}
	if diff != "" || len(changed) != 0 || !empty {
		t.Fatal("symlink refusal returned a partial diff")
	}
	if strings.Contains(err.Error(), key) || strings.Contains(err.Error(), sentinel) {
		t.Fatal("the error quotes the symlink target")
	}
	result, gaps := advRun(t, root, "TASK-ADV-LINK2", nil, changed, diff, empty, err)
	advNoSecret(t, result, gaps, sentinel)
	advNotVerified(t, result)
	if !advHas(gaps, "rigor_check_unavailable") {
		t.Fatalf("types=%v", advTypes(gaps))
	}
}

func TestAdvExactCapTextIsUnavailable(t *testing.T) {
	root := gitRepo(t)
	full := filepath.Join(root, "src", "big.txt")
	if err := os.MkdirAll(filepath.Dir(full), 0o755); err != nil {
		t.Fatal(err)
	}
	f, err := os.Create(full)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := f.Write(bytes.Repeat([]byte{'A'}, 8192)); err != nil {
		t.Fatal(err)
	}
	if err := f.Truncate(gitDiffCap); err != nil {
		t.Fatal(err)
	}
	if err := f.Close(); err != nil {
		t.Fatal(err)
	}
	info, err := os.Stat(full)
	if err != nil {
		t.Fatal(err)
	}
	if info.Size() != gitDiffCap {
		t.Fatalf("size=%d", info.Size())
	}
	_, diff, _, err := verify.CollectGitState(context.Background(), root)
	if err == nil {
		t.Fatalf("file of exactly the cap was returned as a complete diff (diffLen=%d)", len(diff))
	}
	if !strings.Contains(err.Error(), "exceeds") {
		t.Fatalf("err=%v", err)
	}
	if strings.Contains(err.Error(), strings.Repeat("A", 64)) {
		t.Fatal("the cap error quotes the file body")
	}
	result, gaps := advRun(t, root, "TASK-ADV-CAP", []dc.PlannedFile{
		{Path: "src/big.txt", AllowedChange: dc.ChangeCreate},
	}, nil, "", true, err)
	advNotVerified(t, result)
	if !advHas(gaps, "rigor_check_unavailable") {
		t.Fatalf("cap refusal was not unavailable (types=%v status=%s)", advTypes(gaps), result.Status)
	}
}

func TestAdvExactCapBinaryKeyIsNotACleanScan(t *testing.T) {
	key := exampleAWSKey()
	line := []byte("\nlet aws_key = \"" + key + "\";\n")
	root := gitRepo(t)
	full := filepath.Join(root, "src", "big.bin")
	if err := os.MkdirAll(filepath.Dir(full), 0o755); err != nil {
		t.Fatal(err)
	}
	f, err := os.Create(full)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := f.Write([]byte{0}); err != nil {
		t.Fatal(err)
	}
	if err := f.Truncate(gitDiffCap); err != nil {
		t.Fatal(err)
	}
	if _, err := f.WriteAt(line, gitDiffCap-int64(len(line))); err != nil {
		t.Fatal(err)
	}
	if err := f.Close(); err != nil {
		t.Fatal(err)
	}
	info, err := os.Stat(full)
	if err != nil {
		t.Fatal(err)
	}
	if info.Size() != gitDiffCap {
		t.Fatalf("size=%d", info.Size())
	}
	changed, diff, empty, err := verify.CollectGitState(context.Background(), root)
	result, gaps := advRun(t, root, "TASK-ADV-BINCAP", []dc.PlannedFile{
		{Path: "src/big.bin", AllowedChange: dc.ChangeCreate},
	}, changed, diff, empty, err)
	advNoSecret(t, result, gaps)
	advNotVerified(t, result)
	if err == nil && !advHas(gaps, "security_risk") && result.Status != "degraded" && !advHas(gaps, "rigor_check_unavailable") {
		t.Fatalf("binary file at the cap looked complete (types=%v status=%s diffLen=%d)", advTypes(gaps), result.Status, len(diff))
	}
	t.Logf("collect_err=%v status=%s types=%v", err != nil, result.Status, advTypes(gaps))
}

func TestAdvDirtySubmoduleDoesNotFollowSymlink(t *testing.T) {
	key := exampleAWSKey()
	const sentinel = "SYMLINK-TARGET-SENTINEL"
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
			t.Fatalf("git %v: %v %s", args, err, out)
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
	outside := filepath.Join(t.TempDir(), "outside.txt")
	if err := os.WriteFile(outside, []byte(sentinel+"\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(outside, filepath.Join(root, "sub", "link.txt")); err != nil {
		t.Fatal(err)
	}

	changed, diff, empty, err := verify.CollectGitState(context.Background(), root)
	errText := ""
	if err != nil {
		errText = err.Error()
	}
	if strings.Contains(diff, sentinel) || strings.Contains(errText, sentinel) || strings.Contains(errText, key) {
		t.Fatal("symlink target or credential leaked into the collect result")
	}
	result, gaps := advRun(t, root, "TASK-ADV-SUB", []dc.PlannedFile{
		{Path: "sub/s.txt", AllowedChange: dc.ChangeModify},
	}, changed, diff, empty, err)
	advNoSecret(t, result, gaps, sentinel)
	advNotVerified(t, result)
	if err == nil && !advHas(gaps, "security_risk") {
		t.Fatalf("dirty submodule key was not a secret finding (types=%v status=%s)", advTypes(gaps), result.Status)
	}
	t.Logf("collect_err=%v status=%s types=%v", err != nil, result.Status, advTypes(gaps))
}

func TestAdvDirtySubmoduleTrackedSymlinkIsNotFollowed(t *testing.T) {
	key := exampleAWSKey()
	const sentinel = "SYMLINK-TARGET-SENTINEL"
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
			t.Fatalf("git %v: %v %s", args, err, out)
		}
	}
	if err := os.MkdirAll(src, 0o755); err != nil {
		t.Fatal(err)
	}
	run(src, "init", "-q")
	writeFile(t, src, "s.txt", "ok\n")
	writeFile(t, src, "extra.txt", "ok\n")
	run(src, "add", "s.txt", "extra.txt")
	run(src, "commit", "-qm", "s")
	run(root, "-c", "protocol.file.allow=always", "submodule", "add", "-q", src, "sub")
	run(root, "commit", "-qm", "sub")
	writeFile(t, filepath.Join(root, "sub"), "s.txt", "let aws_key = \""+key+"\";\n")
	outside := filepath.Join(t.TempDir(), "outside.txt")
	if err := os.WriteFile(outside, []byte(sentinel+"\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	extra := filepath.Join(root, "sub", "extra.txt")
	if err := os.Remove(extra); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(outside, extra); err != nil {
		t.Fatal(err)
	}
	_, diff, _, err := verify.CollectGitState(context.Background(), root)
	if err == nil {
		t.Fatalf("tracked submodule symlink was included (diffLen=%d)", len(diff))
	}
	if strings.Contains(err.Error(), sentinel) || strings.Contains(err.Error(), key) || strings.Contains(diff, sentinel) {
		t.Fatal("tracked submodule symlink was followed")
	}
	result, gaps := advRun(t, root, "TASK-ADV-SUBLINK", []dc.PlannedFile{
		{Path: "sub/s.txt", AllowedChange: dc.ChangeModify},
	}, nil, "", true, err)
	advNoSecret(t, result, gaps, sentinel)
	advNotVerified(t, result)
}

func TestAdvHostilePathsDoNotMisattribute(t *testing.T) {
	key := exampleAWSKey()
	cases := []struct {
		name string
		rel  string
	}{
		{"newline", "odd\nname.rs"},
		{"quote", "odd\"name.rs"},
		{"dotdot", "up../x.rs"},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			root := gitRepo(t)
			body := []byte("let aws_key = \"" + key + "\";\n")
			full := filepath.Join(root, tc.rel)
			if err := os.MkdirAll(filepath.Dir(full), 0o755); err != nil {
				t.Fatal(err)
			}
			if err := os.WriteFile(full, body, 0o644); err != nil {
				t.Fatal(err)
			}
			changed, diff, empty, err := verify.CollectGitState(context.Background(), root)
			if err != nil {
				if strings.Contains(err.Error(), key) {
					t.Fatal("refusal quotes the credential")
				}
				result, gaps := advRun(t, root, "TASK-ADV-PATH", nil, nil, "", true, err)
				advNoSecret(t, result, gaps)
				advNotVerified(t, result)
				return
			}
			result, gaps := advRun(t, root, "TASK-ADV-PATH", []dc.PlannedFile{
				{Path: tc.rel, AllowedChange: dc.ChangeCreate},
			}, changed, diff, empty, nil)
			advNoSecret(t, result, gaps)
			advNotVerified(t, result)
			for _, g := range gaps {
				if g.GapType == "security_risk" && (g.File == nil || *g.File != tc.rel) {
					got := ""
					if g.File != nil {
						got = *g.File
					}
					t.Fatalf("secret attributed to %q", got)
				}
			}
			if !advHas(gaps, "security_risk") && !advHas(gaps, "rigor_check_unavailable") {
				t.Fatalf("hostile path was neither scanned nor refused (types=%v)", advTypes(gaps))
			}
		})
	}
}

func TestAdvPathContainingSpaceBSlashIsNotRetargeted(t *testing.T) {
	key := exampleAWSKey()
	const rel = "odd b/note.txt"
	root := gitRepo(t)
	writeFile(t, root, "note.txt", "pub fn ok() {}\n")
	writeFile(t, root, rel, "let aws_key = \""+key+"\";\n")
	changed, diff, empty, err := verify.CollectGitState(context.Background(), root)
	if err != nil {
		if strings.Contains(err.Error(), key) {
			t.Fatal("refusal quotes the credential")
		}
		result, gaps := advRun(t, root, "TASK-ADV-SPACEB", []dc.PlannedFile{
			{Path: rel, AllowedChange: dc.ChangeCreate},
		}, nil, "", true, err)
		advNoSecret(t, result, gaps)
		advNotVerified(t, result)
		if !advHas(gaps, "rigor_check_unavailable") {
			t.Fatalf("types=%v", advTypes(gaps))
		}
		return
	}
	result, gaps := advRun(t, root, "TASK-ADV-SPACEB", []dc.PlannedFile{
		{Path: rel, AllowedChange: dc.ChangeCreate},
		{Path: "note.txt", AllowedChange: dc.ChangeCreate},
	}, changed, diff, empty, nil)
	advNoSecret(t, result, gaps)
	advNotVerified(t, result)
	for _, g := range gaps {
		if g.GapType != "security_risk" {
			continue
		}
		if g.File == nil || *g.File != rel {
			got := ""
			if g.File != nil {
				got = *g.File
			}
			t.Fatalf("bytes under %s were attributed to %q", rel, got)
		}
	}
	if !advHas(gaps, "security_risk") && !advHas(gaps, "rigor_check_unavailable") {
		t.Fatalf("path with b/ separator was neither scanned nor refused (types=%v status=%s)", advTypes(gaps), result.Status)
	}
	t.Logf("status=%s types=%v", result.Status, advTypes(gaps))
}
