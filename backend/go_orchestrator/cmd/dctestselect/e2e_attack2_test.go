//go:build e2e

package main

import (
	"bytes"
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// Round-2 attacks that need the real devmap or the real cargo.

// A .rs fixture that no test imports through the graph, read by include_str!.
func TestE2EARustFixtureOutsideSrcAndTestsRunsEverything(t *testing.T) {
	p, _ := scenarioPlan(t, func(r string) {
		put2(r, "rust/ca/fixtures/sample.rs", "pub fn sample() -> i32 {\n    1\n}\n")
		put2(r, "rust/ca/tests/foo.rs", "const D: &str = include_str!(\"../fixtures/sample.rs\");\n#[test]\nfn reads_fixture() {\n    assert!(D.contains(\"sample\"));\n}\n")
	}, func(r string) {
		put2(r, "rust/ca/fixtures/sample.rs", "pub fn renamed() -> i32 {\n    1\n}\n")
	})
	if !coversUnit(p, "rust", "ca") {
		t.Fatalf("fixtures/sample.rs (include_str!'d by ca's foo test) changed; plan selects %q, everything=%t, dropped=%v", cmds(p), p.Everything, p.Dropped)
	}
}

// A fixture .rs under tests/fixtures read by a unit test in src (not by a tests/ target).
func TestE2EARustFixtureUnderTestsSelectsTheWholeCrateIncludingItsLib(t *testing.T) {
	p, _ := scenarioPlan(t, func(r string) {
		put2(r, "rust/ca/tests/fixtures/sample.rs", "pub fn sample() -> i32 {\n    1\n}\n")
		put2(r, "rust/ca/src/lib.rs", "pub fn price(x: i32) -> i32 {\n    x + 1\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn reads() {\n        assert!(include_str!(\"../tests/fixtures/sample.rs\").contains(\"sample\"));\n    }\n}\n")
		put2(r, "rust/ca/tests/foo.rs", "#[test]\nfn f() {\n    assert_eq!(ca::price(1), 2);\n}\n")
	}, func(r string) {
		put2(r, "rust/ca/tests/fixtures/sample.rs", "pub fn renamed() -> i32 {\n    1\n}\n")
	})
	lib := false
	for _, s := range p.Selected {
		for _, u := range s.Units {
			if u.Pkg == "ca" && u.Target == "" {
				lib = true // a whole-crate unit runs the lib
			}
		}
	}
	if !p.Everything && !lib {
		t.Fatalf("tests/fixtures/sample.rs is include_str!'d by a #[cfg(test)] unit test of ca's lib; plan selects %q and never the lib", cmds(p))
	}
}

// A Go file under testdata/ read by a test.
func TestE2EAGoFileUnderTestdataRunsEverything(t *testing.T) {
	p, _ := scenarioPlan(t, func(r string) {
		put2(r, "backend/go_orchestrator/lib/testdata/sample.go", "package sample\n\nfunc A() int {\n\treturn 1\n}\n")
		put2(r, "backend/go_orchestrator/lib/lib_test.go", "package lib\n\nimport (\n\t\"os\"\n\t\"strings\"\n\t\"testing\"\n)\n\nfunc TestName(t *testing.T) {\n\tb, _ := os.ReadFile(\"testdata/sample.go\")\n\tif !strings.Contains(string(b), \"func A\") {\n\t\tt.Fatal(\"x\")\n\t}\n}\n")
	}, func(r string) {
		put2(r, "backend/go_orchestrator/lib/testdata/sample.go", "package sample\n\nfunc B() int {\n\treturn 1\n}\n")
	})
	if !coversUnit(p, "go", "example.com/e2e/lib") {
		t.Fatalf("lib/testdata/sample.go (read by TestName) changed; plan selects %q everything=%t dropped=%v", cmds(p), p.Everything, p.Dropped)
	}
}

// examples/ is compiled by `cargo test`; a change that stops it compiling
// fails the whole command, and the selection never builds it.
func TestE2EACrateExampleChangeRunsEverything(t *testing.T) {
	p, _ := scenarioPlan(t, func(r string) {
		put2(r, "rust/ca/examples/demo.rs", "fn main() {\n    println!(\"{}\", ca::price(1));\n}\n")
	}, func(r string) {
		put2(r, "rust/ca/examples/demo.rs", "fn main() {\n    println!(\"{}\", ca::nope(1));\n}\n")
	})
	if !coversUnit(p, "rust", "ca") {
		t.Fatalf("examples/demo.rs no longer compiles (cargo test builds it); plan selects %q everything=%t dropped=%v", cmds(p), p.Everything, p.Dropped)
	}
}

// A crate outside the cargo workspace (like rust/gusset-engine) that Go links.
func TestE2EANonMemberRustCrateChangeRunsEverything(t *testing.T) {
	p, _ := scenarioPlan(t, func(r string) {
		put2(r, "rust/outside/Cargo.toml", "[package]\nname = \"outside\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n")
		put2(r, "rust/outside/src/lib.rs", "pub fn f() -> i32 {\n    1\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {\n        assert_eq!(super::f(), 1);\n    }\n}\n")
	}, func(r string) {
		put2(r, "rust/outside/src/lib.rs", "pub fn f() -> i32 {\n    2\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {\n        assert_eq!(super::f(), 1);\n    }\n}\n")
	})
	if !p.Everything && len(p.Selected) == 0 {
		t.Fatalf("a change in a non-member crate (rust/gusset-engine is one, linked into Go by cgo) selects nothing and does not widen; dropped=%v widened=%v", p.Dropped, p.Widened)
	}
}

// A #[path] module under src/: libtest names its tests after the `mod`, not the
// file. The file selects the whole crate, and running that whole crate runs the
// renamed module's test, which fails here on purpose.
func TestE2EAPathAttributeModuleChangeSelectsAndRunsTheWholeCrate(t *testing.T) {
	bin := devmapBin(t)
	root := e2eRepo(t)
	put(t, root, "rust/ca/src/lib.rs", mustRead(t, root, "rust/ca/src/lib.rs")+"\n#[path = \"sub/b.rs\"]\nmod renamed;\n")
	put(t, root, "rust/ca/src/sub/b.rs", "#[test]\nfn tb() {\n    assert!(true);\n}\n")
	commit(t, root, "setup")
	put(t, root, "rust/ca/src/sub/b.rs", "#[test]\nfn tb() {\n    panic!(\"the renamed module's test runs only if the crate is selected whole\");\n}\n")
	commit(t, root, "break the renamed module")
	p, _ := planOf(t, root, bin, "HEAD~1")
	whole := findCmd(t, p, "cargo test --locked -p ca --no-fail-fast")
	for _, c := range cmds(p) {
		if strings.Contains(c, " -- ") {
			t.Errorf("a libtest filter was emitted: %q", c)
		}
	}
	var out, errs bytes.Buffer
	code, _ := runPhase(context.Background(), root, []Command{whole.Command}, 25*time.Minute, t.TempDir(), &out, &errs)
	if code != 1 || !strings.Contains(out.String(), "tb") {
		t.Fatalf("exit=%d; running the selected crate whole must fail on the renamed module's test:\n%s", code, out.String())
	}
}

// A src/bin/<tool>/ module belongs to bin `tool`, which only a whole-crate run
// is certain to build and test.
func TestE2EAModuleOfAnAutoDiscoveredBinSelectsTheWholeCrate(t *testing.T) {
	bin := devmapBin(t)
	root := e2eRepo(t)
	put(t, root, "rust/ca/src/bin/tool/main.rs", "mod helper;\nfn main() {\n    helper::h();\n}\n")
	put(t, root, "rust/ca/src/bin/tool/helper.rs", "pub fn h() {}\n")
	commit(t, root, "setup")
	put(t, root, "rust/ca/src/bin/tool/helper.rs", "pub fn h() {\n    println!(\"x\");\n}\n")
	commit(t, root, "change the module")
	p, _ := planOf(t, root, bin, "HEAD~1")
	findCmd(t, p, "cargo test --locked -p ca --no-fail-fast")
}

// The selected-status file from an earlier run, or from another plan, is not
// this run's. run removes it at start, and the audit refuses one whose plan hash
// is not the plan's.
func TestE2EAStaleStatusFromAnotherPlanIsNotReadAsThisRuns(t *testing.T) {
	logDir := t.TempDir()
	plan := Plan{Schema: planSchema, Widened: []Widening{}, Selected: []Selected{}, Dropped: []Dropped{}, Notices: []string{},
		Full: []Command{{Display: "sleep", Dir: ".", Argv: []string{"sleep", "30"}, Log: "full-rust.log"}}}
	useFullSuite(t, plan.Full...)
	pp := filepath.Join(t.TempDir(), "plan.json")
	if err := writeAtomic(pp, plan); err != nil {
		t.Fatal(err)
	}
	stale := phaseStatus{Schema: statusSchema, PlanSHA256: "another-plan", Commands: []ExitInfo{
		{Cmd: "cargo test", Log: "full-rust.log", Completed: true, Detail: "exit status 0"},
		{Cmd: "go test", Log: "full-go.jsonl", Completed: true, Detail: "exit status 0"}}}
	if err := writeAtomic(filepath.Join(logDir, "full-status.json"), stale); err != nil {
		t.Fatal(err)
	}
	// A log cut at a binary boundary, which the parser cannot tell from a short suite.
	if err := os.WriteFile(filepath.Join(logDir, "full-rust.log"), []byte("     Running unittests src/lib.rs (t/debug/deps/a-1)\n\nrunning 1 test\ntest a ... ok\n\ntest result: ok. 1 passed; 0 failed\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(logDir, "full-go.jsonl"), []byte(`{"Action":"pass","Package":"p"}`+"\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	res := filepath.Join(t.TempDir(), "r.json")
	var out bytes.Buffer
	code := run(context.Background(), []string{"audit", "--plan", pp, "--log-dir", logDir, "--out", res, "--sha", "x"}, &out, &out)
	body, _ := os.ReadFile(res)
	if code != 2 || strings.Contains(string(body), `"examined": true`) {
		t.Fatalf("exit=%d; a stale status plus a log cut at a binary boundary audited as examined:\n%s", code, body)
	}
	// And a run killed before it writes its own status leaves none: run removes
	// the stale one at start.
	ctx, cancel := context.WithCancel(context.Background())
	go func() { time.Sleep(500 * time.Millisecond); cancel() }()
	_ = run(ctx, []string{"run", "--repo", ".", "--plan", pp, "--phase", "full", "--log-dir", logDir}, &out, &out)
	st, err := loadStatus(filepath.Join(logDir, "full-status.json"), planSHA(t, pp))
	if err != nil || st.Commands[0].Completed {
		t.Fatalf("the cancelled run's own status must say the command did not complete: %+v err=%v", st, err)
	}
}
