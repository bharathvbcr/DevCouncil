package main

import (
	"bytes"
	"context"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/devmap"
)

// This test runs the real thing end to end: a git repository with two cargo
// crates and a Go package, a real `devmap build`, a real `devmap blast --since`,
// real `cargo metadata` and `go list`. It does not skip. A selector verified
// only against a fake devmap agrees with whatever this build believes devmap
// says, and the field names it reads belong to a binary built elsewhere.
//
// The binary is $MANVI_MAP_BINARY or `devmap` on PATH, as dc/devmap's live
// contract test resolves it. Neither being there fails the test: CI builds it
// first (see selected-tests.yml and analysis-plane.yml), and a missing one is a
// broken job, not an unverified pass.

func devmapBin(t *testing.T) string {
	t.Helper()
	if p := os.Getenv("MANVI_MAP_BINARY"); p != "" {
		if _, err := os.Stat(p); err != nil {
			t.Fatalf("MANVI_MAP_BINARY=%s is not there: %v", p, err)
		}
		return p
	}
	p, err := exec.LookPath("devmap")
	if err != nil {
		t.Fatalf("devmap is not on PATH and MANVI_MAP_BINARY is unset; the selector cannot be verified against the producer: %v", err)
	}
	return p
}

func sh(t *testing.T, dir string, argv ...string) string {
	t.Helper()
	cmd := exec.Command(argv[0], argv[1:]...)
	cmd.Dir = dir
	var out, errs bytes.Buffer
	cmd.Stdout, cmd.Stderr = &out, &errs
	if err := cmd.Run(); err != nil {
		t.Fatalf("%s: %v\n%s%s", strings.Join(argv, " "), err, out.String(), errs.String())
	}
	return strings.TrimSpace(out.String())
}

func put(t *testing.T, root, rel, body string) {
	t.Helper()
	full := filepath.Join(root, filepath.FromSlash(rel))
	if err := os.MkdirAll(filepath.Dir(full), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(full, []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
}

func commit(t *testing.T, root, msg string) {
	t.Helper()
	sh(t, root, "git", "add", "-A")
	sh(t, root, "git", "-c", "user.name=t", "-c", "user.email=t@example.com", "-c", "commit.gpgsign=false",
		"-c", "core.hooksPath=/dev/null", "commit", "-q", "-m", msg)
}

func e2eRepo(t *testing.T) string {
	t.Helper()
	root, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	sh(t, root, "git", "init", "-q", "-b", "main")
	put(t, root, ".gitignore", ".devcouncil/\n.devmap/\ntarget/\n")
	put(t, root, "rust/Cargo.toml", "[workspace]\nresolver = \"2\"\nmembers = [\"ca\", \"cb\"]\n")
	put(t, root, "rust/ca/Cargo.toml", "[package]\nname = \"ca\"\nversion = \"0.1.0\"\nedition = \"2021\"\n")
	put(t, root, "rust/ca/src/lib.rs", "pub fn price(x: i32) -> i32 {\n    x + 1\n}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn prices() {\n        assert_eq!(price(1), 2);\n    }\n}\n")
	put(t, root, "rust/cb/Cargo.toml", "[package]\nname = \"cb\"\nversion = \"0.1.0\"\nedition = \"2021\"\nautotests = false\n\n[dependencies]\nca = { path = \"../ca\" }\n\n[[test]]\nname = \"it\"\npath = \"tests/it.rs\"\n")
	put(t, root, "rust/cb/src/lib.rs", "pub fn total(x: i32) -> i32 {\n    ca::price(x) * 2\n}\n")
	put(t, root, "rust/cb/tests/it.rs", "mod totals;\n")
	put(t, root, "rust/cb/tests/totals.rs", "#[test]\nfn doubles() {\n    assert_eq!(cb::total(1), 4);\n}\n")
	put(t, root, "backend/go_orchestrator/go.mod", "module example.com/e2e\n\ngo 1.24\n")
	put(t, root, "backend/go_orchestrator/lib/lib.go", "package lib\n\nfunc Name() string {\n\treturn \"a\"\n}\n")
	put(t, root, "backend/go_orchestrator/lib/lib_test.go", "package lib\n\nimport \"testing\"\n\nfunc TestName(t *testing.T) {\n\tif Name() == \"\" {\n\t\tt.Fatal(\"empty\")\n\t}\n}\n")
	put(t, root, "backend/go_orchestrator/other/other.go", "package other\n\nfunc Other() int {\n\treturn 1\n}\n")
	put(t, root, "backend/go_orchestrator/other/other_test.go", "package other\n\nimport \"testing\"\n\nfunc TestOther(t *testing.T) {\n\tif Other() != 1 {\n\t\tt.Fatal(\"no\")\n\t}\n}\n")
	sh(t, filepath.Join(root, "rust"), "cargo", "generate-lockfile", "--offline")
	commit(t, root, "base")
	return root
}

func planOf(t *testing.T, root, bin, base string) (Plan, string) {
	t.Helper()
	// Index the head, as the job does before it asks.
	if _, err := devmap.New(bin, root).Build(context.Background(), 5*time.Minute); err != nil {
		t.Fatalf("devmap build: %v", err)
	}
	out := filepath.Join(t.TempDir(), "plan.json")
	var stdout, stderr bytes.Buffer
	code := run(context.Background(), []string{"plan", "--repo", root, "--devmap", bin, "--base", base, "--out", out, "--trigger", "e2e"}, &stdout, &stderr)
	if code != 0 {
		t.Fatalf("plan exited %d: %s", code, stderr.String())
	}
	body, err := os.ReadFile(out)
	if err != nil {
		t.Fatal(err)
	}
	var p Plan
	if err := json.Unmarshal(body, &p); err != nil {
		t.Fatalf("plan.json is not a plan: %v\n%s", err, body)
	}
	return p, stdout.String()
}

func TestEndToEndARustChangeSelectsItsTestsAndAGoChangeSelectsItsPackage(t *testing.T) {
	bin := devmapBin(t)
	root := e2eRepo(t)

	// 1. A Rust change: the body of ca::price.
	put(t, root, "rust/ca/src/lib.rs", strings.Replace(mustRead(t, root, "rust/ca/src/lib.rs"), "x + 1", "x + 2", 1))
	commit(t, root, "price")
	p, log := planOf(t, root, bin, "HEAD~1")
	if p.Everything {
		t.Fatalf("a one-line body change widened to everything: %+v\n%s", p.Widened, log)
	}
	if p.Blast == nil || p.Blast.SeedsTotal == 0 {
		t.Fatalf("the real blast named no seed: %+v", p.Blast)
	}
	t.Logf("rust change: selected=%q dropped=%v notices=%q", cmds(p), p.Dropped, p.Notices)
	ca := findCmd(t, p, "cargo test --locked -p ca --lib --no-fail-fast")
	if len(ca.Seeds) == 0 || !strings.Contains(strings.Join(ca.Seeds, " "), "price") {
		t.Errorf("the selected command does not name the seed that selected it: %v", ca.Seeds)
	}
	for _, s := range p.Selected {
		if strings.HasPrefix(s.Display, "go test") {
			t.Errorf("a Rust-only change selected Go: %q", s.Display)
		}
	}
	if !strings.Contains(log, "::notice::selected cargo test --locked -p ca --lib") {
		t.Errorf("the log does not say what was selected and from which seeds:\n%s", log)
	}

	// 2. A Go change: lib.Name.
	put(t, root, "backend/go_orchestrator/lib/lib.go", strings.Replace(mustRead(t, root, "backend/go_orchestrator/lib/lib.go"), `"a"`, `"b"`, 1))
	commit(t, root, "name")
	p, log = planOf(t, root, bin, "HEAD~1")
	if p.Everything {
		t.Fatalf("a Go body change widened to everything: %+v\n%s", p.Widened, log)
	}
	findCmd(t, p, "go test -count=1 example.com/e2e/lib")
	for _, s := range p.Selected {
		if strings.HasPrefix(s.Display, "cargo") || strings.Contains(s.Display, "other") {
			t.Errorf("an unrelated unit was selected: %q", s.Display)
		}
	}
}

func TestEndToEndAnUnreadableDevmapRunsEverything(t *testing.T) {
	bin := devmapBin(t)
	root := e2eRepo(t)
	put(t, root, "rust/ca/src/lib.rs", strings.Replace(mustRead(t, root, "rust/ca/src/lib.rs"), "x + 1", "x + 3", 1))
	commit(t, root, "price")
	if _, err := devmap.New(bin, root).Build(context.Background(), 5*time.Minute); err != nil {
		t.Fatal(err)
	}
	out := filepath.Join(t.TempDir(), "plan.json")
	var stdout bytes.Buffer
	code := run(context.Background(), []string{"plan", "--repo", root, "--devmap", filepath.Join(t.TempDir(), "no-such-devmap"), "--base", "HEAD~1", "--out", out}, &stdout, &bytes.Buffer{})
	if code != 0 {
		t.Fatalf("plan exited %d; a failed question is a wide plan, not a failed step", code)
	}
	var p Plan
	body, err := os.ReadFile(out)
	if err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal(body, &p); err != nil {
		t.Fatal(err)
	}
	wantEverything(t, p, "devmap blast failed")
	if len(p.Full) != 2 {
		t.Errorf("the full suite must still be planned: %+v", p.Full)
	}
}

func TestEndToEndAChangeToCargoLockRunsEverything(t *testing.T) {
	bin := devmapBin(t)
	root := e2eRepo(t)
	put(t, root, "rust/Cargo.lock", mustRead(t, root, "rust/Cargo.lock")+"\n# touched\n")
	put(t, root, "rust/ca/src/lib.rs", strings.Replace(mustRead(t, root, "rust/ca/src/lib.rs"), "x + 1", "x + 4", 1))
	commit(t, root, "lock")
	p, _ := planOf(t, root, bin, "HEAD~1")
	wantEverything(t, p, "rust/Cargo.lock")
}

func TestResolveBaseFollowsTheEventAndFailsClosed(t *testing.T) {
	root := e2eRepo(t)
	first := sh(t, root, "git", "rev-parse", "HEAD")
	sh(t, root, "git", "update-ref", "refs/remotes/origin/main", "HEAD")
	sh(t, root, "git", "checkout", "-q", "-b", "topic")
	put(t, root, "rust/ca/src/lib.rs", mustRead(t, root, "rust/ca/src/lib.rs")+"\n// topic\n")
	commit(t, root, "topic")
	second := sh(t, root, "git", "rev-parse", "HEAD")
	ctx := context.Background()
	run := execRunner{timeout: time.Minute}
	base := func(o Options) (string, error) {
		o.Repo = root
		if o.BaseRef == "" {
			o.BaseRef = "main"
		}
		b, _, err := resolveBase(ctx, run, o)
		return b, err
	}
	// A branch push and a pull request both measure from the merge base.
	if got, err := base(Options{Event: "push", RefName: "topic", Before: second}); err != nil || got != first {
		t.Errorf("branch push: base=%s err=%v, want the merge base %s", got, err, first)
	}
	if got, err := base(Options{Event: "pull_request", RefName: "7/merge"}); err != nil || got != first {
		t.Errorf("pull request: base=%s err=%v, want the merge base %s", got, err, first)
	}
	// A push to the default branch measures from the commit before it.
	if got, err := base(Options{Event: "push", RefName: "main", Before: first}); err != nil || got != first {
		t.Errorf("default-branch push: base=%s err=%v", got, err)
	}
	// A new ref (all-zero before) falls back to the merge base.
	if got, err := base(Options{Event: "push", RefName: "main", Before: strings.Repeat("0", 40)}); err != nil || got != first {
		t.Errorf("new ref: base=%s err=%v", got, err)
	}
	// Everything that cannot be resolved is an error, which the plan turns into a full run.
	for name, o := range map[string]Options{
		"force push, before absent": {Event: "push", RefName: "main", Before: strings.Repeat("a", 40)},
		"option-shaped base":        {Base: "--output=/tmp/x"},
		"missing origin branch":     {Event: "pull_request", BaseRef: "nope"},
		"option-shaped base ref":    {Event: "pull_request", BaseRef: "-x"},
	} {
		if got, err := base(o); err == nil {
			t.Errorf("%s: resolved to %q; want an error", name, got)
		}
	}
}

func mustRead(t *testing.T, root, rel string) string {
	t.Helper()
	b, err := os.ReadFile(filepath.Join(root, filepath.FromSlash(rel)))
	if err != nil {
		t.Fatal(err)
	}
	return string(b)
}

func findCmd(t *testing.T, p Plan, display string) Selected {
	t.Helper()
	for _, s := range p.Selected {
		if s.Display == display {
			return s
		}
	}
	t.Fatalf("selection lacks %q; it is %q (everything=%t widened=%v dropped=%v)", display, cmds(p), p.Everything, p.Widened, p.Dropped)
	return Selected{}
}
