package main

import (
	"bytes"
	"context"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/devmap"
)

// Hardening tests from the adversarial round: each asserts the behaviour the
// attack showed was missing.

// Adversarial tests. A FAIL here is a reproduced break.

func realShapeWorkspace() *RustWorkspace {
	t := func(name, src string, kinds ...string) RustTarget {
		return RustTarget{Name: name, Kinds: kinds, Src: src, Test: true, Doctest: true}
	}
	return &RustWorkspace{DriverClosure: map[string]bool{"devmap-cli": true, "devmap-store": true, "devmap-query": true, "devmap-extract": true}, Packages: []RustPackage{
		{Name: "devmap-store", Dir: "rust/devmap-store", Targets: []RustTarget{t("devmap_store", "rust/devmap-store/src/lib.rs", "lib")}},
		{Name: "devmap-query", Dir: "rust/devmap-query", Targets: []RustTarget{t("devmap_query", "rust/devmap-query/src/lib.rs", "lib")}},
		{Name: "devmap-extract", Dir: "rust/devmap-extract", Targets: []RustTarget{t("devmap_extract", "rust/devmap-extract/src/lib.rs", "lib")}},
		{Name: "devmap-cli", Dir: "rust/devmap-cli", Targets: []RustTarget{t("devmap", "rust/devmap-cli/src/main.rs", "bin")}},
	}}
}

// ATTACK 1a: the cross-process rule only fires for dc-* and devmap-cli. The
// `devmap` binary Go tests run is built from devmap-cli AND its path
// dependencies (devmap-extract/query/resolve/store/analyze/serve, dc-regress).
func TestADevmapLibraryChangeSelectsTheGoPackagesThatRunDevmap(t *testing.T) {
	in := inputs(withSeed("rust/devmap-extract/src/lib.rs", "extract"), withChanged("rust/devmap-extract/src/lib.rs"),
		withTests("rust/devmap-extract/src/lib.rs"))
	in.Rust = realShapeWorkspace()
	p := BuildPlan(in)
	for _, s := range p.Selected {
		for _, u := range s.Units {
			if u.Kind == "go" {
				return
			}
		}
	}
	if p.Everything {
		return
	}
	t.Fatalf("a devmap-extract change (compiled into the `devmap` binary dc/devmap and repomap tests exec) selects only %q; no Go package, no widening", cmds(p))
}

// rust/devmap-store/src/db.rs declares
//
//	#[path = "db/tests/payload_serialization_tests.rs"] mod payload_serialization_tests;
//
// so libtest names those tests db::payload_serialization_tests::..., not after
// the file's path, and a filter built from the path matches nothing. Deriving
// every #[path] is not cheap, so the selection carries a backstop: the filtered
// command names its unfiltered form, and run re-runs it whole when the filtered
// run ran no test (TestAFilteredRunThatRanNoTestIsRerunWhole).
func TestAPathAttributeModulesFilterFallsBackToTheWholeTarget(t *testing.T) {
	in := inputs(withSeed("rust/devmap-store/src/db/tests/payload_serialization_tests.rs", "t"),
		withTests("rust/devmap-store/src/db/tests/payload_serialization_tests.rs"))
	in.Rust = realShapeWorkspace()
	p := BuildPlan(in)
	var filtered *Selected
	for i, s := range p.Selected {
		if len(s.Units) == 1 && s.Units[0].Pkg == "devmap-store" && len(s.Units[0].Filters) > 0 {
			filtered = &p.Selected[i]
		}
	}
	if filtered == nil {
		t.Fatalf("no filtered command for devmap-store: %q", cmds(p))
	}
	want := "cargo test --locked -p devmap-store --lib --no-fail-fast"
	if got := strings.Join(filtered.Fallback, " "); got != want {
		t.Fatalf("fallback = %q, want %q", got, want)
	}
	for _, s := range p.Selected {
		if len(s.Units[0].Filters) == 0 && s.Fallback != nil {
			t.Errorf("an unfiltered command has a fallback: %q", s.Display)
		}
	}
}

func filtersOf(p Plan) [][]string {
	var out [][]string
	for _, s := range p.Selected {
		for _, u := range s.Units {
			out = append(out, u.Filters)
		}
	}
	return out
}

// ATTACK 1c: doc tests are a separate cargo target (`--doc`); a lib-source
// change that breaks a doctest is never selected.
func TestALibrarySelectionAlsoSelectsItsDoctests(t *testing.T) {
	p := BuildPlan(inputs(withSeed("rust/ca/src/engine.rs", "run"), withTests("rust/ca/src/engine.rs")))
	for _, s := range p.Selected {
		for _, u := range s.Units {
			if u.Pkg == "ca" && (u.Target == "" || u.Target == "--doc") {
				return
			}
		}
	}
	t.Fatalf("selection %q has no --doc for ca; a changed doc example is invisible to the fast phase", cmds(p))
}

// ATTACK 1d: a changed path that git reports but the blast does not (only
// everythingTrigger sees it) and that is not in the trigger list.
func TestAGitOnlyPathOfUnknownKindRunsEverything(t *testing.T) {
	in := inputs(withSeed("rust/ca/src/engine.rs", "run"), withTests("rust/ca/tests/foo.rs"))
	in.GitChanged = []string{"rust/ca/src/engine.rs", "rust/.cargo/config.toml", "rust/clippy.toml", "rust/deny.toml", "rust/rustfmt.toml", ".cargo/config", "rust/.config/nextest.toml"}
	p := BuildPlan(in)
	if !p.Everything {
		t.Fatalf("git reports rust/.cargo/config.toml changed but the blast did not list it: plan = %q widened=%v (the only backstop is devmap's file_not_indexed)", cmds(p), p.Widened)
	}
}

// ATTACK 1e: go.sum alone, plus the other triggers, are HELD; keep as proof.
func TestBuildDefinitionsRunEverything(t *testing.T) {
	for _, f := range []string{"backend/go_orchestrator/go.sum", "rust/Cargo.toml", "rust/ca/build.rs", "rust/rust-toolchain.toml", "rust/testsupport/x.rs", "backend/go_orchestrator/go.work"} {
		in := inputs(withSeed("rust/ca/src/engine.rs", "run"))
		in.GitChanged = []string{f}
		if p := BuildPlan(in); !p.Everything {
			t.Errorf("%s did not widen", f)
		}
	}
}

// ATTACK 2: hostile blast documents.
func TestHostileBlastDocumentsAreRefused(t *testing.T) {
	head := strings.Repeat("a", 40)
	base := strings.Repeat("b", 40)
	ok := func(extra string) string {
		return `{"change":"` + base + `..` + head + `","complete":true,"changed_files":[],"seeds":[],"impacted":[],"tests":[` + extra + `],"unattributed":[],"unavailable":[]}`
	}
	cases := map[string]string{
		"empty":                 "",
		"whitespace":            "  \n",
		"truncated":             ok("")[:60],
		"array":                 "[]",
		"abs test path":         ok(`{"distance":1,"path":"/etc/passwd"}`),
		"dotdot test path":      ok(`{"distance":1,"path":"rust/../../x_test.go"}`),
		"negative distance":     ok(`{"distance":-1,"path":"a/b_test.go"}`),
		"NaN distance":          ok(`{"distance":NaN,"path":"a/b_test.go"}`),
		"string distance":       ok(`{"distance":"1","path":"a/b_test.go"}`),
		"null tests":            strings.Replace(ok(""), `"tests":[]`, `"tests":null`, 1),
		"missing unavailable":   strings.Replace(ok(""), `,"unavailable":[]`, "", 1),
		"windows path":          ok(`{"distance":1,"path":"rust\\ca\\tests\\foo.rs"}`),
		"trailing garbage":      ok("") + `{"x":1}`,
		"complete true, no env": `{"complete":true}`,
	}
	for name, doc := range cases {
		b, err := devmap.DecodeBlast([]byte(doc))
		if name == "trailing garbage" {
			// json.Unmarshal rejects trailing data; document the behaviour.
			if err == nil {
				t.Errorf("%s: decoded", name)
			}
			continue
		}
		if err == nil {
			t.Errorf("%s: DecodeBlast accepted it: %+v", name, b)
		}
	}
	// A duplicated key: the last wins; a complete:false then complete:true pair
	// reads as complete. Plan ignores `complete`, so it only affects the artifact.
	dup := strings.Replace(ok(""), `"complete":true`, `"complete":false,"complete":true`, 1)
	if b, err := devmap.DecodeBlast([]byte(dup)); err != nil || !b.Complete {
		t.Logf("duplicate key: err=%v", err)
	}
	// complete:true while `unavailable` lists a limit and `unattributed` is
	// empty: Plan must not trust `complete`.
	lie := strings.Replace(ok(""), `"unavailable":[]`, `"unavailable":[{"kind":"change_unattributed","lines":1,"ranges":1}]`, 1)
	b, err := devmap.DecodeBlast([]byte(lie))
	if err != nil {
		t.Fatal(err)
	}
	in := inputs()
	in.Blast = b
	in.Blast.Change = baseSHA + ".." + headSHA
	if p := BuildPlan(in); !p.Everything {
		t.Errorf("complete:true with a change_unattributed entry and no ranges did not widen: %q", cmds(p))
	}
}

// ATTACK 2b: huge arrays. A blast with a very large seeds list must not blow
// the log or the artifact.
func TestAHugeSeedListIsBounded(t *testing.T) {
	var opts []blastOpt
	for i := 0; i < 50000; i++ {
		opts = append(opts, withSeed("rust/ca/src/engine.rs", "f"+strconv.Itoa(i)))
	}
	in := inputs(opts...)
	p := BuildPlan(in)
	body, _ := jsonIndent(p)
	if len(body) > 2<<20 {
		t.Fatalf("plan.json is %d bytes for 50000 seeds", len(body))
	}
	for _, s := range p.Selected {
		if len(s.Seeds) > maxSeedsPerCmd+1 {
			t.Fatalf("a command lists %d seeds", len(s.Seeds))
		}
	}
}

// ATTACK 4: libtest parser.
const libtestHead = "     Running unittests src/lib.rs (target/debug/deps/zz-1)\n\nrunning 2 tests\n"

func TestAFooterPlantedInATestsStdoutDoesNotMoveAFailure(t *testing.T) {
	log := "     Running unittests src/lib.rs (target/debug/deps/zz-1)\n\nrunning 2 tests\ntest a ... ok\ntest b ... FAILED\n\nfailures:\n\n---- b stdout ----\n" +
		"error: test failed, to rerun pass `-p ca --lib`\n\nfailures:\n    b\n\ntest result: FAILED. 1 passed; 1 failed; 0 ignored\n\n" +
		"error: test failed, to rerun pass `-p zz --lib`\n"
	r := parseLibtest(strings.NewReader(log))
	if len(r.Failures) != 1 || r.Failures[0].Pkg != "zz" {
		t.Fatalf("failure of zz was attributed to %+v (a test's own stdout carried a cargo footer for ca)", r.Failures)
	}
}

func TestAFailedLinePlantedInATestsStdoutIsNotAFailure(t *testing.T) {
	log := "     Running unittests src/lib.rs (target/debug/deps/zz-1)\n\nrunning 1 test\ntest b ... FAILED\n\nfailures:\n\n---- b stdout ----\ntest ghost::phantom ... FAILED\n\nfailures:\n    b\n\ntest result: FAILED. 0 passed; 1 failed\n\nerror: test failed, to rerun pass `-p zz --lib`\n"
	r := parseLibtest(strings.NewReader(log))
	if len(r.Failures) != 1 {
		t.Fatalf("one test failed, the parser reports %d: %+v", len(r.Failures), r.Failures)
	}
}

func TestAnAbortedTestBinaryIsNotExamined(t *testing.T) {
	log := "     Running unittests src/lib.rs (target/debug/deps/ok-1)\n\nrunning 1 test\ntest a ... ok\n\ntest result: ok. 1 passed; 0 failed\n\n" +
		"     Running unittests src/lib.rs (target/debug/deps/zz-1)\n\nrunning 1 test\nerror: test failed, to rerun pass `-p zz --lib`\n\nCaused by:\n  process didn't exit successfully: `/x/zz-1` (signal: 6, SIGABRT: process abort signal)\n"
	r := parseLibtest(strings.NewReader(log))
	if r.Examined && len(r.Failures) == 0 {
		t.Fatalf("a test binary that died by SIGABRT is invisible: examined=%t failures=%v", r.Examined, r.Failures)
	}
}

func TestATruncatedLibtestLogIsNotExamined(t *testing.T) {
	log := "     Running unittests src/lib.rs (target/debug/deps/ok-1)\n\nrunning 1 test\ntest a ... ok\n\ntest result: ok. 1 passed; 0 failed\n\n     Running unittests src/lib.rs (target/debug/deps/zz-1)\n\nrunning 40 tests\ntest x ... ok\ntest y ... ok\ntest z"
	r := parseLibtest(strings.NewReader(log))
	if r.Examined {
		t.Fatalf("a log cut mid-target (killed or timed out) is Examined=true")
	}
}

func TestAnsiColouredLibtestIsStillParsed(t *testing.T) {
	log := "     Running unittests src/lib.rs (target/debug/deps/zz-1)\n\nrunning 1 test\ntest b ... \x1b[31mFAILED\x1b[0m\n\ntest result: \x1b[31mFAILED\x1b[0m. 0 passed; 1 failed\n\n\x1b[1m\x1b[31merror\x1b[0m: test failed, to rerun pass `-p zz --lib`\n"
	r := parseLibtest(strings.NewReader(log))
	if len(r.Failures) == 0 {
		t.Fatalf("CARGO_TERM_COLOR=always hides every failure: examined=%t note=%q failures=0", r.Examined, r.Note)
	}
}

func TestAFailedLineGluedToUnterminatedOutputIsStillFound(t *testing.T) {
	log := "     Running unittests src/lib.rs (target/debug/deps/zz-1)\n\nrunning 1 test\nprogress...test zz::b ... FAILED\n\ntest result: FAILED. 0 passed; 1 failed\n\nerror: test failed, to rerun pass `-p zz --lib`\n"
	r := parseLibtest(strings.NewReader(log))
	if len(r.Failures) != 1 {
		t.Fatalf("a print! without newline before libtest's result line hides the failure: %v", r.Failures)
	}
}

func TestGoJSONEdgeCases(t *testing.T) {
	buildFail := `{"Action":"output","Package":"p","Output":"FAIL p [build failed]\n"}` + "\n" + `{"Action":"fail","Package":"p"}` + "\n"
	r := parseGoJSON(strings.NewReader(buildFail))
	if len(r.Failures) != 1 || r.Failures[0].Name != "[package]" {
		t.Errorf("build failure: %+v", r)
	}
	outputOnly := `{"Action":"output","Package":"p","Output":"x\n"}` + "\n"
	if r := parseGoJSON(strings.NewReader(outputOnly)); r.Examined {
		t.Errorf("output-only stream examined")
	}
	cut := `{"Action":"pass","Package":"p"}` + "\n" + `{"Action":"fa`
	if r := parseGoJSON(strings.NewReader(cut)); r.Examined {
		t.Errorf("mid-line cut examined")
	}
	// killed on a line boundary after one package finished
	killed := `{"Action":"pass","Package":"p"}` + "\n" + `{"Action":"run","Package":"q","Test":"TestX"}` + "\n"
	if r := parseGoJSON(strings.NewReader(killed)); r.Examined {
		t.Errorf("BREAK: go log killed mid-package on a line boundary is Examined=true: %+v", r)
	}
}

// ATTACK 5: recall math.
func TestAnEverythingPlanHasNullRecall(t *testing.T) {
	in := AuditInput{
		Plan: Plan{Schema: 1, Everything: true},
		Rust: SuiteRun{Examined: true, Failures: []Failure{{Kind: "rust", Pkg: "ca", Target: "--lib", Name: "x"}}},
		Go:   SuiteRun{Examined: true},
		Base: []string{},
	}
	r, _ := Audit(in)
	if r.Recall != nil {
		t.Fatalf("recall = %v for a plan that selected EVERYTHING; a widened run says nothing about DevMap's recall", *r.Recall)
	}
}

func TestABaseArtifactFromAnotherRefEventOrSchemaIsRefused(t *testing.T) {
	// A feature branch's own artifact (trigger pull_request, other sha) that
	// lists every test as failing would hide every miss if it were read as the base.
	body := []byte(`{"schema":1,"sha":"feature","trigger":"pull_request","full":{"rust":{"examined":true},"go":{"examined":true},"failed":["rust:ca:--lib:x"]}}`)
	got, _, why := BaseFailures(body, mainRef)
	if got != nil {
		t.Fatalf("accepted an artifact from trigger=pull_request sha=feature as the base's failures (why=%q); Recall has no ref field to check", why)
	}
}

// ATTACK 6/7: injection into the runner's workflow-command channel, and the
// process-group kill.
func TestAFileNameCannotInjectAWorkflowCommand(t *testing.T) {
	evil := "rust/ca/src/a\n::warning::injected.rs"
	p := BuildPlan(inputs(withSeed(evil, "f"), withTests(evil)))
	if len(p.Selected) == 0 {
		t.Skipf("nothing selected: %v", p.Dropped)
	}
	var out bytes.Buffer
	for _, s := range p.Selected {
		runAll(context.Background(), ".", []Command{{Display: s.Display, Dir: ".", Argv: helper("ok").Argv}}, time.Minute, "", &out, &out)
	}
	for _, line := range strings.Split(out.String(), "\n") {
		if strings.HasPrefix(line, "::warning::") {
			t.Fatalf("a file name made the runner see a workflow command: %q", line)
		}
	}
}
