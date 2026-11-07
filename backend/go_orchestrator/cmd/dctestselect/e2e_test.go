//go:build e2e

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

// Built only with -tags e2e (selected-tests.yml and ci:local pass it): these
// tests run devmap, cargo and go on temp repositories, and analysis-plane's
// `go test ./...` on three platforms and under -race should not pay for them.
// The tag is explicit; nothing here skips.
//
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
	// A stand-in for rust/gusset-engine: its own workspace, depending on a crate
	// that is not in the main one. The plan reads its dependencies from here.
	put(t, root, "rust/gusset-engine/Cargo.toml", "[package]\nname = \"engine\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n\n[dependencies]\ndc-glob = { path = \"../dc-glob\" }\n")
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

func TestE2EARustChangeSelectsItsTestsAndAGoChangeSelectsItsPackage(t *testing.T) {
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
	ca := findCmd(t, p, "cargo test --locked -p ca -p cb --no-fail-fast")
	if len(ca.Seeds) == 0 || !strings.Contains(strings.Join(ca.Seeds, " "), "price") {
		t.Errorf("the selected command does not name the seed that selected it: %v", ca.Seeds)
	}
	for _, s := range p.Selected {
		if strings.HasPrefix(s.Display, "go test") {
			t.Errorf("a Rust-only change selected Go: %q", s.Display)
		}
	}
	if !strings.Contains(log, "::notice::selected cargo test --locked -p ca -p cb --no-fail-fast") {
		t.Errorf("the log does not say what was selected and from which seeds:\n%s", log)
	}

	// 2. A Go change: lib.Name.
	put(t, root, "backend/go_orchestrator/lib/lib.go", strings.Replace(mustRead(t, root, "backend/go_orchestrator/lib/lib.go"), `"a"`, `"b"`, 1))
	commit(t, root, "name")
	p, log = planOf(t, root, bin, "HEAD~1")
	if p.Everything {
		t.Fatalf("a Go body change widened to everything: %+v\n%s", p.Widened, log)
	}
	findCmd(t, p, "go test -count=1 -json example.com/e2e/lib")
	for _, s := range p.Selected {
		if strings.HasPrefix(s.Display, "cargo") || strings.Contains(s.Display, "other") {
			t.Errorf("an unrelated unit was selected: %q", s.Display)
		}
	}
}

func TestE2EAnUnreadableDevmapRunsEverything(t *testing.T) {
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

func TestE2EAChangeToCargoLockRunsEverything(t *testing.T) {
	bin := devmapBin(t)
	root := e2eRepo(t)
	put(t, root, "rust/Cargo.lock", mustRead(t, root, "rust/Cargo.lock")+"\n# touched\n")
	put(t, root, "rust/ca/src/lib.rs", strings.Replace(mustRead(t, root, "rust/ca/src/lib.rs"), "x + 1", "x + 4", 1))
	commit(t, root, "lock")
	p, _ := planOf(t, root, bin, "HEAD~1")
	wantEverything(t, p, "rust/Cargo.lock")
}

func TestE2EResolveBaseFollowsTheEventAndFailsClosed(t *testing.T) {
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

// ATTACK 2c: a devmap that exits 0 and prints nothing, via the real plan
// command with a fake binary.
func TestE2EDevmapExitsZeroPrintingNothing(t *testing.T) {
	root := e2eRepo(t)
	put2(root, "rust/ca/src/lib.rs", strings.Replace(mustRead(t, root, "rust/ca/src/lib.rs"), "x + 1", "x + 9", 1))
	commit(t, root, "c")
	fake := filepath.Join(t.TempDir(), "devmap")
	if err := os.WriteFile(fake, []byte("#!/bin/sh\nexit 0\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	out := filepath.Join(t.TempDir(), "plan.json")
	var so, se bytes.Buffer
	if code := run(context.Background(), []string{"plan", "--repo", root, "--devmap", fake, "--base", "HEAD~1", "--out", out}, &so, &se); code != 0 {
		t.Fatalf("exit %d %s", code, se.String())
	}
	p, _, err := loadPlan(out)
	if err != nil {
		t.Fatal(err)
	}
	wantEverything(t, p, "devmap blast failed")
}

// ATTACK 2d: an index older than HEAD. The index is built at HEAD~1; HEAD has
// one more commit. Does plan notice, or does blast refresh the index itself?
func TestE2EStaleIndexIsNotTrusted(t *testing.T) {
	bin := devmapBin(t)
	root := e2eRepo(t)
	put2(root, "rust/ca/src/lib.rs", strings.Replace(mustRead(t, root, "rust/ca/src/lib.rs"), "x + 1", "x + 2", 1))
	commit(t, root, "c1")
	if _, err := devmap.New(bin, root).Build(context.Background(), 5*time.Minute); err != nil {
		t.Fatal(err)
	}
	put2(root, "rust/cb/src/lib.rs", strings.Replace(mustRead(t, root, "rust/cb/src/lib.rs"), "* 2", "* 3", 1))
	commit(t, root, "c2")
	out := filepath.Join(t.TempDir(), "plan.json")
	var so, se bytes.Buffer
	if code := run(context.Background(), []string{"plan", "--repo", root, "--devmap", bin, "--base", "HEAD~2", "--out", out}, &so, &se); code != 0 {
		t.Fatalf("exit %d %s", code, se.String())
	}
	p, _, _ := loadPlan(out)
	t.Logf("stale index: everything=%t selected=%q widened=%v blast=%+v", p.Everything, cmds(p), p.Widened, p.Blast)
	if !p.Everything && !coversUnit(p, "rust", "cb") {
		t.Fatalf("index built at HEAD~1, change in HEAD (cb) is missing from the selection: %q", cmds(p))
	}
}

// ATTACK 3: base resolution.
func TestE2EBaseEdgeCases(t *testing.T) {
	root := e2eRepo(t)
	ctx := context.Background()
	r := execRunner{timeout: time.Minute}
	first := sh(t, root, "git", "rev-parse", "HEAD")
	sh(t, root, "git", "update-ref", "refs/remotes/origin/main", "HEAD")

	// shallow clone: the commit before the push is absent.
	put2(root, "x.txt", "1\n")
	commit(t, root, "two")
	second := sh(t, root, "git", "rev-parse", "HEAD")
	shallow := filepath.Join(t.TempDir(), "s")
	sh(t, root, "git", "clone", "-q", "--depth", "1", "file://"+root, shallow)
	sh(t, shallow, "git", "update-ref", "refs/remotes/origin/main", "HEAD")
	if _, _, err := resolveBase(ctx, r, Options{Repo: shallow, Event: "push", RefName: "main", BaseRef: "main", Before: first}); err == nil {
		t.Errorf("shallow clone: before %s resolved though it is not in the clone", first)
	}
	// merge-base in a shallow clone with a diverged origin/main: fails or lies?
	sh(t, shallow, "git", "checkout", "-q", "-b", "topic")
	put2(shallow, "y.txt", "1\n")
	commit(t, shallow, "topic")
	base, _, err := resolveBase(ctx, r, Options{Repo: shallow, Event: "push", RefName: "topic", BaseRef: "main"})
	t.Logf("shallow topic: base=%s err=%v (origin/main=%s)", base, err, second)

	// workflow_dispatch on main: HEAD == origin/main, so the change is empty.
	sh(t, root, "git", "update-ref", "refs/remotes/origin/main", "HEAD")
	b, h, err := resolveBase(ctx, r, Options{Repo: root, Event: "workflow_dispatch", RefName: "main", BaseRef: "main"})
	if err != nil || b == h || b != first {
		t.Errorf("dispatch on main: base=%s head=%s err=%v; want the parent %s, not an empty change", b, h, err, first)
	}
	// before = a ref name rather than a sha.
	if _, _, err := resolveBase(ctx, r, Options{Repo: root, Event: "push", RefName: "main", BaseRef: "main", Before: "main"}); err == nil {
		t.Errorf("a non-sha before id resolved")
	}
	_ = second
}

// Attack tests (adversarial lane): each scenario changes a file a test depends
// on in a way the graph may not see, runs the real devmap + cargo metadata +
// go list, and requires the plan to either widen or select a command that
// covers the dependent test. A plan that selects nothing relevant and does not
// widen is silent under-selection.

func coversUnit(p Plan, kind, pkg string) bool {
	if p.Everything {
		return true
	}
	for _, s := range p.Selected {
		for _, u := range s.Units {
			if u.Kind == kind && u.Pkg == pkg {
				return true
			}
		}
	}
	return false
}

func scenarioPlan(t *testing.T, setup, mutate func(root string)) (Plan, string) {
	t.Helper()
	bin := devmapBin(t)
	root := e2eRepo(t)
	setup(root)
	commit(t, root, "setup")
	mutate(root)
	commit(t, root, "mutate")
	p, log := planOf(t, root, bin, "HEAD~1")
	t.Logf("everything=%t selected=%q widened=%v dropped=%v blast.unavailable=%s", p.Everything, cmds(p), p.Widened, p.Dropped, func() string {
		if p.Blast == nil {
			return "-"
		}
		return string(mustJSON(p.Blast.Unavailable))
	}())
	return p, log
}

func mustJSON(v any) []byte {
	b, _ := jsonIndent(v)
	return []byte(strings.ReplaceAll(string(b), "\n", ""))
}

func caDataTest(root string) {
	put2(root, "rust/ca/tests/foo.rs", "const D: &str = include_str!(\"data/x.json\");\n#[test]\nfn reads_data() {\n    assert_eq!(D.trim(), \"1\");\n}\n")
	put2(root, "rust/ca/tests/data/x.json", "1\n")
}

func TestE2ENonRustFileUnderCrateTestsDirIsNotSilentlyIgnored(t *testing.T) {
	p, _ := scenarioPlan(t, func(r string) { caDataTest(r) }, func(r string) { put2(r, "rust/ca/tests/data/x.json", "2\n") })
	if !coversUnit(p, "rust", "ca") {
		t.Fatalf("tests/data/x.json (include_str!'d by ca's foo test) changed; plan selects %q and did not widen", cmds(p))
	}
}

func TestE2ESQLFileIncludedBySourceIsNotSilentlyIgnored(t *testing.T) {
	p, _ := scenarioPlan(t, func(r string) {
		put2(r, "rust/ca/src/q.sql", "select 1;\n")
		put2(r, "rust/ca/src/lib.rs", "pub const Q: &str = include_str!(\"q.sql\");\npub fn price(x: i32) -> i32 {\n    x + 1\n}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n    #[test]\n    fn prices() {\n        assert_eq!(price(1), 2);\n        assert!(Q.starts_with(\"select\"));\n    }\n}\n")
	}, func(r string) { put2(r, "rust/ca/src/q.sql", "selec 1;\n") })
	if !coversUnit(p, "rust", "ca") {
		t.Fatalf("src/q.sql changed; plan selects %q and did not widen", cmds(p))
	}
}

func TestE2ECargoConfigAtRepoRootWidens(t *testing.T) {
	p, _ := scenarioPlan(t, func(r string) { put2(r, "pad.txt", "1\n") }, func(r string) { put2(r, ".cargo/config.toml", "[build]\nrustflags=[\"--cfg\",\"x\"]\n") })
	if !p.Everything {
		t.Fatalf(".cargo/config.toml changed; plan selects %q and did not widen", cmds(p))
	}
}

func TestE2ECargoConfigInWorkspaceWidens(t *testing.T) {
	p, _ := scenarioPlan(t, func(r string) { put2(r, "pad.txt", "1\n") }, func(r string) { put2(r, "rust/.cargo/config.toml", "[env]\nFOO=\"1\"\n") })
	if !p.Everything {
		t.Fatalf("rust/.cargo/config.toml changed; plan selects %q and did not widen", cmds(p))
	}
}

func TestE2EGoTestdataFileIsNotSilentlyIgnored(t *testing.T) {
	p, _ := scenarioPlan(t, func(r string) {
		put2(r, "backend/go_orchestrator/lib/testdata/in.txt", "a\n")
		put2(r, "backend/go_orchestrator/lib/lib_test.go", "package lib\n\nimport (\n\t\"os\"\n\t\"testing\"\n)\n\nfunc TestName(t *testing.T) {\n\tb, _ := os.ReadFile(\"testdata/in.txt\")\n\tif string(b) != \"a\\n\" {\n\t\tt.Fatal(\"x\")\n\t}\n}\n")
	}, func(r string) { put2(r, "backend/go_orchestrator/lib/testdata/in.txt", "b\n") })
	if !coversUnit(p, "go", "example.com/e2e/lib") {
		t.Fatalf("lib/testdata/in.txt changed; plan selects %q and did not widen", cmds(p))
	}
}

func TestE2EGoEmbedFileIsNotSilentlyIgnored(t *testing.T) {
	p, _ := scenarioPlan(t, func(r string) {
		put2(r, "backend/go_orchestrator/lib/emb.txt", "a\n")
		put2(r, "backend/go_orchestrator/lib/lib.go", "package lib\n\nimport _ \"embed\"\n\n//go:embed emb.txt\nvar Emb string\n\nfunc Name() string {\n\treturn \"a\" + Emb\n}\n")
	}, func(r string) { put2(r, "backend/go_orchestrator/lib/emb.txt", "b\n") })
	if !coversUnit(p, "go", "example.com/e2e/lib") {
		t.Fatalf("lib/emb.txt (go:embed) changed; plan selects %q and did not widen", cmds(p))
	}
}

func TestE2EScriptReadByATestIsNotSilentlyIgnored(t *testing.T) {
	p, _ := scenarioPlan(t, func(r string) {
		put2(r, "scripts/helper.py", "print(1)\n")
		put2(r, "rust/ca/tests/foo.rs", "#[test]\nfn runs_script() {\n    let p = concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/../../scripts/helper.py\");\n    assert!(std::fs::read_to_string(p).unwrap().contains(\"print\"));\n}\n")
	}, func(r string) { put2(r, "scripts/helper.py", "pass\n") })
	if !coversUnit(p, "rust", "ca") {
		t.Fatalf("scripts/helper.py changed (read by ca's foo test); plan selects %q and did not widen", cmds(p))
	}
}

func TestE2EDocsOnlyChange(t *testing.T) {
	p, _ := scenarioPlan(t, func(r string) { put2(r, "README.md", "a\n") }, func(r string) { put2(r, "README.md", "b\n") })
	t.Logf("README-only: everything=%t selected=%q", p.Everything, cmds(p))
}

func TestE2ERenamedTestFile(t *testing.T) {
	bin := devmapBin(t)
	root := e2eRepo(t)
	put2(root, "rust/ca/tests/foo.rs", "#[test]\nfn f() {\n    assert_eq!(ca::price(1), 2);\n}\n")
	commit(t, root, "setup")
	sh(t, root, "git", "mv", "rust/ca/tests/foo.rs", "rust/ca/tests/foo2.rs")
	put2(root, "rust/ca/tests/foo2.rs", "#[test]\nfn f() {\n    assert_eq!(ca::price(1), 3);\n}\n")
	commit(t, root, "rename+edit")
	p, _ := planOf(t, root, bin, "HEAD~1")
	t.Logf("rename: everything=%t selected=%q dropped=%v widened=%v", p.Everything, cmds(p), p.Dropped, p.Widened)
	if !coversUnit(p, "rust", "ca") {
		t.Fatalf("renamed+edited test: plan selects %q", cmds(p))
	}
}

func TestE2EDeletedSourceFile(t *testing.T) {
	bin := devmapBin(t)
	root := e2eRepo(t)
	put2(root, "rust/ca/src/util.rs", "pub fn u() -> i32 {\n    1\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn u_is_one() {\n        assert_eq!(super::u(), 1);\n    }\n}\n")
	put2(root, "rust/ca/src/lib.rs", "pub mod util;\n"+mustRead(t, root, "rust/ca/src/lib.rs"))
	commit(t, root, "setup")
	sh(t, root, "git", "rm", "-q", "rust/ca/src/util.rs")
	put2(root, "rust/ca/src/lib.rs", strings.TrimPrefix(mustRead(t, root, "rust/ca/src/lib.rs"), "pub mod util;\n"))
	commit(t, root, "delete")
	p, _ := planOf(t, root, bin, "HEAD~1")
	t.Logf("delete: everything=%t selected=%q dropped=%v widened=%v", p.Everything, cmds(p), p.Dropped, p.Widened)
	if !coversUnit(p, "rust", "ca") {
		t.Fatalf("deleted module: plan selects %q", cmds(p))
	}
}

func TestE2ECrossCrateMove(t *testing.T) {
	bin := devmapBin(t)
	root := e2eRepo(t)
	put2(root, "rust/ca/src/util.rs", "pub fn u() -> i32 {\n    1\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn u_is_one() {\n        assert_eq!(super::u(), 1);\n    }\n}\n")
	put2(root, "rust/ca/src/lib.rs", "pub mod util;\n"+mustRead(t, root, "rust/ca/src/lib.rs"))
	commit(t, root, "setup")
	sh(t, root, "git", "mv", "rust/ca/src/util.rs", "rust/cb/src/util.rs")
	put2(root, "rust/ca/src/lib.rs", strings.TrimPrefix(mustRead(t, root, "rust/ca/src/lib.rs"), "pub mod util;\n"))
	put2(root, "rust/cb/src/lib.rs", "pub mod util;\n"+mustRead(t, root, "rust/cb/src/lib.rs"))
	commit(t, root, "move")
	p, _ := planOf(t, root, bin, "HEAD~1")
	t.Logf("move: everything=%t selected=%q dropped=%v widened=%v", p.Everything, cmds(p), p.Dropped, p.Widened)
	if !coversUnit(p, "rust", "cb") {
		t.Fatalf("module moved into cb with its #[cfg(test)] tests; plan selects %q", cmds(p))
	}
}

func TestE2EUnicodePathInTestdata(t *testing.T) {
	p, _ := scenarioPlan(t, func(r string) {
		put2(r, "rust/ca/tests/data/é à.json", "1\n")
		put2(r, "rust/ca/tests/foo.rs", "const D: &str = include_str!(\"data/é à.json\");\n#[test]\nfn d() {\n    assert_eq!(D.trim(), \"1\");\n}\n")
	}, func(r string) { put2(r, "rust/ca/tests/data/é à.json", "2\n") })
	if !coversUnit(p, "rust", "ca") {
		t.Fatalf("unicode/space testdata changed; plan selects %q", cmds(p))
	}
}

func put2(root, rel, body string) {
	t := &testing.T{}
	_ = t
	putNoT(root, rel, body)
}

func putNoT(root, rel, body string) {
	full := filepath.Join(root, filepath.FromSlash(rel))
	if err := os.MkdirAll(filepath.Dir(full), 0o755); err != nil {
		panic(err)
	}
	if err := os.WriteFile(full, []byte(body), 0o644); err != nil {
		panic(err)
	}
}

func TestE2EIndexedScriptReadByATestIsNotSilentlyIgnored(t *testing.T) {
	for _, f := range []struct{ rel, a, b string }{
		{"scripts/helper.py", "def helper():\n    return 1\n", "def helper():\n    return 2\n"},
		{"scripts/helper.mjs", "export function helper() {\n  return 1;\n}\n", "export function helper() {\n  return 2;\n}\n"},
	} {
		f := f
		t.Run(f.rel, func(t *testing.T) {
			p, _ := scenarioPlan(t, func(r string) {
				put2(r, f.rel, f.a)
				put2(r, "rust/ca/tests/foo.rs", "#[test]\nfn runs_script() {\n    let p = concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/../../"+f.rel+"\");\n    let _ = std::process::Command::new(\"true\").arg(p).status();\n}\n")
			}, func(r string) { put2(r, f.rel, f.b) })
			if !coversUnit(p, "rust", "ca") {
				t.Fatalf("%s changed (spawned by ca's foo test); plan selects %q and did not widen: the tests are never reached", f.rel, cmds(p))
			}
		})
	}
}

func TestE2EThePlanHonoursItsTimeoutForADevmapThatHangs(t *testing.T) {
	root := e2eRepo(t)
	put(t, root, "rust/ca/src/lib.rs", strings.Replace(mustRead(t, root, "rust/ca/src/lib.rs"), "x + 1", "x + 5", 1))
	commit(t, root, "c")
	hang := filepath.Join(t.TempDir(), "devmap")
	if err := os.WriteFile(hang, []byte("#!/bin/sh\nsleep 120\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	out := filepath.Join(t.TempDir(), "plan.json")
	started := time.Now()
	var so bytes.Buffer
	code := run(context.Background(), []string{"plan", "--repo", root, "--devmap", hang, "--base", "HEAD~1", "--timeout", "2s", "--out", out}, &so, &bytes.Buffer{})
	if code != 0 || time.Since(started) > 60*time.Second {
		t.Fatalf("exit=%d after %s; a 2s timeout was not honoured", code, time.Since(started))
	}
	p, _, err := loadPlan(out)
	if err != nil {
		t.Fatal(err)
	}
	wantEverything(t, p, "devmap blast failed")
}

func TestE2ETheDriverClosureFromRealCargoMetadata(t *testing.T) {
	root, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	sh(t, root, "git", "init", "-q", "-b", "main")
	put(t, root, "rust/Cargo.toml", "[workspace]\nresolver = \"2\"\nmembers = [\"devmap-cli\", \"devmap-lib\", \"devmap-testonly\", \"unrelated\"]\n")
	crate := func(name, deps string) {
		put(t, root, "rust/"+name+"/Cargo.toml", "[package]\nname = \""+name+"\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"+deps)
		file := "lib.rs"
		if name == "devmap-cli" {
			file = "main.rs"
		}
		body := "pub fn f() {}\n"
		if name == "devmap-cli" {
			body = "fn main() {}\n"
		}
		put(t, root, "rust/"+name+"/src/"+file, body)
	}
	crate("devmap-cli", "\n[dependencies]\ndevmap-lib = { path = \"../devmap-lib\" }\n")
	crate("devmap-lib", "\n[dev-dependencies]\ndevmap-testonly = { path = \"../devmap-testonly\" }\n")
	crate("devmap-testonly", "")
	crate("unrelated", "")
	sh(t, filepath.Join(root, "rust"), "cargo", "generate-lockfile", "--offline")
	ws, err := loadRust(context.Background(), execRunner{timeout: 2 * time.Minute}, root, "rust")
	if err != nil {
		t.Fatal(err)
	}
	for _, in := range []string{"devmap-cli", "devmap-lib"} {
		if !ws.DriverClosure[in] {
			t.Errorf("%s is in the closure of the devmap binary, closure = %v", in, ws.DriverClosure)
		}
	}
	for _, out := range []string{"devmap-testonly", "unrelated"} {
		if ws.DriverClosure[out] {
			t.Errorf("%s is not compiled into the binary, closure = %v", out, ws.DriverClosure)
		}
	}
}
