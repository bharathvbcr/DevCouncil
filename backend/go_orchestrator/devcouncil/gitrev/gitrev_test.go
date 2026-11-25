package gitrev

import (
	"context"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
)

type stubRunner map[string]string

func (s stubRunner) Output(_ context.Context, _ string, argv ...string) ([]byte, error) {
	key := strings.Join(argv, " ")
	out, ok := s[key]
	if !ok {
		return nil, errors.New("unexpected command: " + key)
	}
	return []byte(out + "\n"), nil
}

func TestValidationHelpers(t *testing.T) {
	sha40 := strings.Repeat("a", 40)
	sha64 := strings.Repeat("f", 64)
	if !IsSHA40(sha40) || IsSHA40(sha64) || IsSHA40(strings.Repeat("A", 40)) || IsSHA40("main") {
		t.Fatalf("IsSHA40 mismatch")
	}
	if !IsFullSHA(sha40) || !IsFullSHA(sha64) || IsFullSHA("short") {
		t.Fatalf("IsFullSHA mismatch")
	}
	if !IsZeroSHA(strings.Repeat("0", 40)) || IsZeroSHA(sha40) || IsZeroSHA("") {
		t.Fatalf("IsZeroSHA mismatch")
	}
	if !IsRefToken("origin/main") || !IsRefToken("HEAD~1") {
		t.Fatalf("expected valid ref tokens")
	}
	for _, bad := range []string{"", "--exec=evil", "-n", "main;rm -rf /", "a\nb", "a b", "$(id)"} {
		if IsRefToken(bad) {
			t.Errorf("IsRefToken(%q) = true, want false", bad)
		}
	}
}

func TestResolveRevAndMergeBase(t *testing.T) {
	base := strings.Repeat("1", 40)
	head := strings.Repeat("2", 40)
	mb := strings.Repeat("3", 40)
	run := stubRunner{
		"git rev-parse --verify --quiet main^{commit}": base,
		"git merge-base " + base + " " + head:          mb,
	}
	got, err := ResolveRev(context.Background(), run, ".", "main")
	if err != nil || got != base {
		t.Fatalf("ResolveRev = %q, %v; want %q", got, err, base)
	}
	if _, err := ResolveRev(context.Background(), run, ".", "--upload-pack=evil"); err == nil {
		t.Fatal("expected option-like revision to be refused")
	}
	gotMB, err := MergeBase(context.Background(), run, ".", base, head)
	if err != nil || gotMB != mb {
		t.Fatalf("MergeBase = %q, %v; want %q", gotMB, err, mb)
	}
	if _, err := MergeBase(context.Background(), run, ".", "-x", head); err == nil {
		t.Fatal("expected option-like base to be refused")
	}
}

func TestResolveRevSessionOnRealGitRepo(t *testing.T) {
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
	if err := os.WriteFile(filepath.Join(dir, "a.txt"), []byte("one\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	runGit("add", "a.txt")
	runGit("commit", "-m", "base")
	base := runGit("rev-parse", "HEAD")

	if err := os.WriteFile(filepath.Join(dir, "a.txt"), []byte("two\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	runGit("add", "a.txt")
	runGit("commit", "-m", "head")
	head := runGit("rev-parse", "HEAD")

	sess, err := proc.NewGitSession(dir)
	if err != nil {
		t.Fatal(err)
	}
	gotHead, err := ResolveRevSession(context.Background(), sess, "HEAD")
	if err != nil || gotHead != head {
		t.Fatalf("ResolveRevSession(HEAD) = %q, %v; want %q", gotHead, err, head)
	}
	gotMB, err := MergeBaseSession(context.Background(), sess, base, head)
	if err != nil || gotMB != base {
		t.Fatalf("MergeBaseSession = %q, %v; want %q", gotMB, err, base)
	}
}

func TestCheckRepoPath(t *testing.T) {
	for _, good := range []string{"a.go", "dir/sub/file.rs", ".github/workflows/ci.yml", ".gitattributes"} {
		if err := CheckRepoPath(good); err != nil {
			t.Errorf("CheckRepoPath(%q) = %v, want nil", good, err)
		}
	}
	for _, bad := range []string{"", "/etc/passwd", "../secret", "a/../b", "./a", "a//b", "a\\b", "C:foo", "a\x00b"} {
		if err := CheckRepoPath(bad); err == nil {
			t.Errorf("CheckRepoPath(%q) = nil, want error", bad)
		}
	}
}

func TestWorkflowEscaping(t *testing.T) {
	hostileFile := "dir/foo:bar,baz%0A\r\n::error::pwn.go"
	hostileMsg := "bad%value\r\n::set-env name=X::1"

	got := AnnotateAt("error", hostileFile, 42, "gate:title,1", hostileMsg)
	if strings.ContainsAny(got, "\r\n") {
		t.Fatalf("annotation contains raw line break: %q", got)
	}
	wantPrefix := "::error file=dir/foo%3Abar%2Cbaz%250A%0D%0A%3A%3Aerror%3A%3Apwn.go,line=42,title=gate%3Atitle%2C1::"
	if !strings.HasPrefix(got, wantPrefix) {
		t.Fatalf("got  %q\nwant prefix %q", got, wantPrefix)
	}
	wantSuffix := "bad%25value%0D%0A::set-env name=X::1"
	if !strings.HasSuffix(got, wantSuffix) {
		t.Fatalf("got  %q\nwant suffix %q", got, wantSuffix)
	}

	if g := Group("run\n::endgroup::"); strings.Contains(g, "\n") || g != "::group::run%0A::endgroup::" {
		t.Fatalf("Group = %q", g)
	}
	if a := Annotate("warning", "a%b\rc\nd"); a != "::warning::a%25b%0Dc%0Ad" {
		t.Fatalf("Annotate = %q", a)
	}
	// Unknown level is clamped to notice so untrusted callers cannot forge command names.
	if a := Annotate("add-mask", "secret"); !strings.HasPrefix(a, "::notice::") {
		t.Fatalf("unknown level was not clamped: %q", a)
	}
}
