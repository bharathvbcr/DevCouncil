package main

import (
	"errors"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/devmap"
)

const (
	headSHA = "1111111111111111111111111111111111111111"
	baseSHA = "2222222222222222222222222222222222222222"
	module  = "example.com/m"
)

// workspace is a small stand-in for the real one, with every shape the mapper
// has a rule for: a lib plus a src/main.rs bin and an isolated test (ca), a
// harness crate with autotests off (cb), a bin-only crate (devmap-cli), a
// crate the job excludes (dc-store), and a target that needs a feature (cf).
func workspace() *RustWorkspace {
	t := func(name, src string, kinds ...string) RustTarget {
		return RustTarget{Name: name, Kinds: kinds, Src: src, Test: true, Doctest: true}
	}
	return &RustWorkspace{Packages: []RustPackage{
		{Name: "ca", Dir: "rust/ca", Targets: []RustTarget{
			t("ca", "rust/ca/src/lib.rs", "lib"), t("ca", "rust/ca/src/main.rs", "bin"),
			t("foo", "rust/ca/tests/foo.rs", "test"), t("bar", "rust/ca/tests/bar.rs", "test"),
		}},
		{Name: "cb", Dir: "rust/cb", Targets: []RustTarget{
			t("cb", "rust/cb/src/lib.rs", "lib"), t("it", "rust/cb/tests/it.rs", "test"),
		}},
		{Name: "cc", Dir: "rust/cc", Targets: []RustTarget{
			t("cc", "rust/cc/src/lib.rs", "lib"), t("deep", "rust/cc/tests/deep/main.rs", "test"),
		}},
		{Name: "cf", Dir: "rust/cf", Targets: []RustTarget{
			t("cf", "rust/cf/src/lib.rs", "lib"),
			{Name: "gated", Kinds: []string{"test"}, Src: "rust/cf/tests/gated.rs", Test: true, RequiredFeatures: []string{"slow"}},
		}},
		{Name: "devmap-cli", Dir: "rust/devmap-cli", Targets: []RustTarget{
			t("devmap", "rust/devmap-cli/src/main.rs", "bin"), t("it", "rust/devmap-cli/tests/it.rs", "test"),
		}},
		{Name: "dc-store", Dir: "rust/dc-store", Targets: []RustTarget{t("dc_store", "rust/dc-store/src/lib.rs", "lib")}},
	}}
}

func goWorkspace() *GoWorkspace {
	return &GoWorkspace{Module: module, Packages: []GoPackage{
		{ImportPath: module + "/pkga", Dir: "go/pkga", TestFiles: []string{"go/pkga/a_test.go"}, GoFiles: []string{"go/pkga/a.go"}},
		{ImportPath: module + "/drv", Dir: "go/drv", TestFiles: []string{"go/drv/d_test.go"}, Imports: []string{module + "/testsupport", "testing"}},
		{ImportPath: module + "/viadevmap", Dir: "go/viadevmap", TestFiles: []string{"go/viadevmap/v_test.go"}, Imports: []string{module + "/dc/devmap"}},
		{ImportPath: module + "/plain", Dir: "go/plain", TestFiles: []string{"go/plain/p_test.go"}, Imports: []string{"strings"}},
		{ImportPath: module + "/testsupport", Dir: "go/testsupport", GoFiles: []string{"go/testsupport/t.go"}},
	}}
}

type blastOpt func(*devmap.Blast)

func withTests(paths ...string) blastOpt {
	return func(b *devmap.Blast) {
		for i, p := range paths {
			b.Tests = append(b.Tests, devmap.BlastTest{Distance: i, Path: p})
		}
	}
}

func withSeed(file, name string) blastOpt {
	return func(b *devmap.Blast) {
		b.Seeds = append(b.Seeds, devmap.BlastSeed{FilePath: file, QualifiedName: file + "::" + name, StartLine: 1, EndLine: 2})
	}
}

func withLimit(kind string, depth, reached int) blastOpt {
	return func(b *devmap.Blast) {
		b.Unavailable = append(b.Unavailable, devmap.BlastUnavailable{Kind: kind, Depth: &depth, Reached: &reached, Raw: []byte(`{"kind":"` + kind + `"}`)})
	}
}

func withUnattributed(path, reason string) blastOpt {
	return func(b *devmap.Blast) {
		b.Unattributed = append(b.Unattributed, devmap.BlastUnattributed{Path: path, Reason: reason})
		b.Unavailable = append(b.Unavailable, devmap.BlastUnavailable{Kind: "change_unattributed", Raw: []byte(`{"kind":"change_unattributed"}`)})
	}
}

func withChanged(paths ...string) blastOpt {
	return func(b *devmap.Blast) {
		for _, p := range paths {
			b.ChangedFiles = append(b.ChangedFiles, devmap.BlastChangedFile{Path: p, Status: "modified"})
		}
	}
}

// inputs is a healthy set of answers; each test changes one thing.
func inputs(opts ...blastOpt) Inputs {
	b := &devmap.Blast{Change: baseSHA + ".." + headSHA}
	for _, o := range opts {
		o(b)
	}
	files := map[string]string{"rust/cb/tests/it.rs": "mod m;\nmod other;\n"}
	return Inputs{
		Base: baseSHA, Head: headSHA, Trigger: "push", Blast: b,
		Rust: workspace(), Go: goWorkspace(), Excludes: DefaultRustExcludes, RustDir: "rust", GoDir: "go",
		ReadFile: func(rel string) ([]byte, error) {
			if s, ok := files[rel]; ok {
				return []byte(s), nil
			}
			return nil, os.ErrNotExist
		},
	}
}

func cmds(p Plan) []string {
	var out []string
	for _, s := range p.Selected {
		out = append(out, s.Display)
	}
	return out
}

func wantCmd(t *testing.T, p Plan, want string) Selected {
	t.Helper()
	for _, s := range p.Selected {
		if s.Display == want {
			return s
		}
	}
	t.Fatalf("selection lacks %q; it is %q (everything=%t, widened=%v)", want, cmds(p), p.Everything, p.Widened)
	return Selected{}
}

func wantEverything(t *testing.T, p Plan, reasonPart string) {
	t.Helper()
	if !p.Everything {
		t.Fatalf("selection = %q; want everything (%s)", cmds(p), reasonPart)
	}
	if len(p.Selected) != 0 {
		t.Errorf("an everything plan must not also list commands: %q", cmds(p))
	}
	if len(p.Widened) == 0 || p.Widened[0].Scope != "everything" || !strings.Contains(p.Widened[0].Reason, reasonPart) {
		t.Errorf("widened = %+v; want an everything entry naming %q", p.Widened, reasonPart)
	}
}

func TestASourceOnlyChangeSelectsTheModuleInTheLibAndTheMainBinary(t *testing.T) {
	p := BuildPlan(inputs(
		withSeed("rust/ca/src/engine.rs", "run"),
		withTests("rust/ca/src/engine.rs")))
	if p.Everything {
		t.Fatalf("a source change with a complete mapping must not widen: %v", p.Widened)
	}
	lib := wantCmd(t, p, "cargo test --locked -p ca --lib --no-fail-fast -- engine::")
	wantCmd(t, p, "cargo test --locked -p ca --bin ca --no-fail-fast -- engine::")
	if lib.Dir != "rust" || len(lib.Units) != 1 || !reflect.DeepEqual(lib.Units[0], Unit{Kind: "rust", Pkg: "ca", Target: "--lib", Filters: []string{"engine::"}}) {
		t.Errorf("unit = %+v dir=%q", lib.Units, lib.Dir)
	}
	if len(lib.Seeds) != 1 || lib.Seeds[0] != "rust/ca/src/engine.rs::run" || len(lib.Files) != 1 {
		t.Errorf("a selected command must name the files and the seeds it came from: files=%v seeds=%v", lib.Files, lib.Seeds)
	}
}

func TestAModuleFileBecomesItsPathAsAFilterAndModRsIsItsDirectory(t *testing.T) {
	p := BuildPlan(inputs(withTests("rust/ca/src/a/b.rs", "rust/ca/src/c/mod.rs", "rust/ca/src/lib.rs")))
	// lib.rs is the crate root: the whole target, which absorbs the filtered runs.
	wantCmd(t, p, "cargo test --locked -p ca --lib --no-fail-fast")
	for _, c := range cmds(p) {
		if strings.Contains(c, "--lib") && strings.Contains(c, "--") && strings.Contains(c, "a::b::") {
			t.Errorf("a target that runs whole must not also run filtered: %q", c)
		}
	}
	p = BuildPlan(inputs(withTests("rust/ca/src/a/b.rs", "rust/ca/src/c/mod.rs")))
	wantCmd(t, p, "cargo test --locked -p ca --lib --no-fail-fast -- a::b:: c::")
}

func TestAHarnessModuleRunsThroughTheItTargetWithItsPrefix(t *testing.T) {
	p := BuildPlan(inputs(withTests("rust/cb/tests/m.rs")))
	wantCmd(t, p, "cargo test --locked -p cb --test it --no-fail-fast -- m::")
}

func TestAHarnessModuleNoHarnessDeclaresRunsEveryTestTarget(t *testing.T) {
	p := BuildPlan(inputs(withTests("rust/cb/tests/orphan.rs")))
	wantCmd(t, p, "cargo test --locked -p cb --test it --no-fail-fast")
}

func TestAnIsolatedTargetRunsAsItsOwnTest(t *testing.T) {
	p := BuildPlan(inputs(withTests("rust/ca/tests/foo.rs", "rust/ca/tests/bar.rs")))
	wantCmd(t, p, "cargo test --locked -p ca --test bar --test foo --no-fail-fast")
}

func TestASupportDirectoryRunsEveryTestTargetOfItsCrate(t *testing.T) {
	p := BuildPlan(inputs(withTests("rust/ca/tests/support/mod.rs")))
	wantCmd(t, p, "cargo test --locked -p ca --test bar --test foo --no-fail-fast")
}

func TestATargetDirectoryRunsThatTarget(t *testing.T) {
	p := BuildPlan(inputs(withTests("rust/cc/tests/deep/helper.rs")))
	wantCmd(t, p, "cargo test --locked -p cc --test deep --no-fail-fast")
}

func TestABinOnlyCratesSourceRunsThroughItsBinary(t *testing.T) {
	p := BuildPlan(inputs(withTests("rust/devmap-cli/src/hook.rs")))
	wantCmd(t, p, "cargo test --locked -p devmap-cli --bin devmap --no-fail-fast -- hook::")
	for _, c := range cmds(p) {
		if strings.Contains(c, "--lib") {
			t.Errorf("devmap-cli has no lib target, so --lib would be a cargo error: %q", c)
		}
	}
}

func TestAGoTestFileRunsItsPackage(t *testing.T) {
	p := BuildPlan(inputs(withSeed("go/pkga/a.go", "Do"), withTests("go/pkga/a_test.go")))
	got := wantCmd(t, p, "go test -count=1 -json "+module+"/pkga")
	if got.Dir != "go" || len(got.Seeds) != 1 || len(got.Files) != 1 {
		t.Errorf("dir=%q seeds=%v files=%v", got.Dir, got.Seeds, got.Files)
	}
}

func TestPathsOutsideTheJobsFullSuiteAreDroppedAndLoggedNeverRun(t *testing.T) {
	p := BuildPlan(inputs(withTests(
		"rust/dc-store/src/lib.rs",  // excluded package
		"rust/cf/tests/gated.rs",    // needs a feature the suite does not enable
		"rust/elsewhere/tests/x.rs", // not a workspace member
		"go/nowhere/n_test.go",      // not a go list package
		"docs/notes.md",             // not a test file
		"rust/ca/benches/b.rs",      // neither tests/ nor src/
		"rust/ca/src/engine.rs")))
	wantCmd(t, p, "cargo test --locked -p ca --lib --no-fail-fast -- engine::")
	reasons := map[string]string{}
	for _, d := range p.Dropped {
		reasons[d.Path] = d.Reason
	}
	for path, part := range map[string]string{
		"rust/dc-store/src/lib.rs":  "excluded",
		"rust/cf/tests/gated.rs":    "requires features",
		"rust/elsewhere/tests/x.rs": "outside every cargo workspace member",
		"go/nowhere/n_test.go":      "no `go list` package",
		"docs/notes.md":             "neither a Rust nor a Go test file",
		"rust/ca/benches/b.rs":      "not under tests/ or src/",
	} {
		if !strings.Contains(reasons[path], part) {
			t.Errorf("%s: reason = %q, want it to say %q", path, reasons[path], part)
		}
	}
	for _, c := range cmds(p) {
		if strings.Contains(c, "dc-store") || strings.Contains(c, "-p cf") {
			t.Errorf("a command outside the full suite's universe was selected: %q", c)
		}
	}
}

func TestChangeUnattributedRunsEveryTargetOfTheOwningCrate(t *testing.T) {
	p := BuildPlan(inputs(
		withUnattributed("rust/ca/src/lib.rs", "outside_every_symbol"),
		withLimit("impact_incomplete", 3, 238),
		withTests("go/pkga/a_test.go")))
	if p.Everything {
		t.Fatalf("an unattributed range widens to its crate, not to everything: %v", p.Widened)
	}
	wantCmd(t, p, "cargo test --locked -p ca --no-fail-fast")
	wantCmd(t, p, "go test -count=1 -json "+module+"/pkga")
	found := false
	for _, w := range p.Widened {
		found = found || (w.Scope == "crate ca" && strings.Contains(w.Reason, "rust/ca/src/lib.rs"))
	}
	if !found {
		t.Errorf("the widening to crate ca was not logged: %+v", p.Widened)
	}
}

func TestChangeUnattributedInAGoFileRunsThatPackage(t *testing.T) {
	p := BuildPlan(inputs(withUnattributed("go/pkga/a.go", "outside_every_symbol")))
	wantCmd(t, p, "go test -count=1 -json "+module+"/pkga")
}

func TestChangeUnattributedInAnExcludedCrateIsLoggedAsDroppedNotWidened(t *testing.T) {
	p := BuildPlan(inputs(
		withUnattributed("rust/dc-store/src/lib.rs", "outside_every_symbol"),
		withTests("rust/ca/src/engine.rs")))
	if p.Everything {
		t.Fatalf("widened to everything: %v", p.Widened)
	}
	if len(p.Dropped) != 1 || p.Dropped[0].Path != "rust/dc-store/src/lib.rs" {
		t.Errorf("dropped = %+v", p.Dropped)
	}
	wantCmd(t, p, "cargo test --locked -p ca --lib --no-fail-fast -- engine::")
}

func TestAChangeOutsideAnySymbolWithNoOwnerRunsEverything(t *testing.T) {
	wantEverything(t, BuildPlan(inputs(withUnattributed("tools/orphan.rs", "outside_every_symbol"))), "no owning test unit")
}

func TestAnUnindexedFileRunsEverything(t *testing.T) {
	wantEverything(t, BuildPlan(inputs(withUnattributed("rust/ca/src/new.xyz", "file_not_indexed"), withTests("rust/ca/src/engine.rs"))), "not indexed")
}

func TestImpactIncompleteIsLoggedWithItsDepthAndDoesNotWiden(t *testing.T) {
	p := BuildPlan(inputs(withLimit("impact_incomplete", 3, 238), withTests("rust/ca/src/engine.rs")))
	if p.Everything || len(p.Widened) != 0 {
		t.Fatalf("impact_incomplete widened the selection: everything=%t widened=%v", p.Everything, p.Widened)
	}
	wantCmd(t, p, "cargo test --locked -p ca --lib --no-fail-fast -- engine::")
	if len(p.Notices) != 1 || !strings.Contains(p.Notices[0], "depth=3 reached=238") || !strings.Contains(p.Notices[0], "full suite gates") {
		t.Errorf("notices = %q; want depth and reached, and who gates", p.Notices)
	}
}

func TestEveryRunEverythingPathRunsEverything(t *testing.T) {
	for _, path := range []string{
		"Cargo.toml", "rust/ca/Cargo.toml", "rust/Cargo.lock", "rust-toolchain.toml", "rust-toolchain",
		"rust/ca/build.rs", ".github/workflows/rust.yml", ".github/actions/setup-rust/action.yml",
		"rust/testsupport/restriction.rs", "rust/testdata/accuracy/go.labels.json", "rust/vendor/tree-sitter/lib.c",
		"backend/go_orchestrator/go.mod", "go.sum",
	} {
		t.Run(path, func(t *testing.T) {
			// Reported by git only: the blast need not list it.
			in := inputs(withTests("rust/ca/src/engine.rs"))
			in.GitChanged = []string{path}
			wantEverything(t, BuildPlan(in), path)
			// Reported by the blast only: git need not list it.
			wantEverything(t, BuildPlan(inputs(withChanged(path), withTests("rust/ca/src/engine.rs"))), path)
		})
	}
}

func TestAnOrdinarySourceChangeIsNotARunEverythingPath(t *testing.T) {
	in := inputs(withTests("rust/ca/src/engine.rs"))
	in.GitChanged = []string{"rust/ca/src/engine.rs", "backend/go_orchestrator/pkga/a.go"}
	p := BuildPlan(in)
	if p.Everything {
		t.Fatalf("widened on ordinary files: %v", p.Widened)
	}
	wantCmd(t, p, "cargo test --locked -p ca --lib --no-fail-fast -- engine::")
}

func TestAnUnknownOrCappedLimitRunsEverything(t *testing.T) {
	for _, kind := range []string{"change_unreadable", "changed_files_capped", "seeds_capped", "budget_exhausted", "a_kind_from_a_newer_devmap"} {
		t.Run(kind, func(t *testing.T) {
			wantEverything(t, BuildPlan(inputs(withLimit(kind, 1, 1), withTests("rust/ca/src/engine.rs"))), kind)
		})
	}
}

func TestChangeUnattributedWithoutItsRangesIsAContradictionAndRunsEverything(t *testing.T) {
	in := inputs(withTests("rust/ca/src/engine.rs"))
	in.Blast.Unavailable = append(in.Blast.Unavailable, devmap.BlastUnavailable{Kind: "change_unattributed", Raw: []byte(`{"kind":"change_unattributed"}`)})
	wantEverything(t, BuildPlan(in), "disagree")
}

func TestAnyFailedQuestionRunsEverything(t *testing.T) {
	boom := errors.New("boom")
	for name, mutate := range map[string]func(*Inputs){
		"base":       func(in *Inputs) { in.BaseErr = boom },
		"blast":      func(in *Inputs) { in.BlastErr = boom },
		"no blast":   func(in *Inputs) { in.Blast = nil },
		"git":        func(in *Inputs) { in.GitErr = boom },
		"cargo":      func(in *Inputs) { in.RustErr = boom },
		"no cargo":   func(in *Inputs) { in.Rust = nil },
		"go list":    func(in *Inputs) { in.GoErr = boom },
		"no go list": func(in *Inputs) { in.Go = nil },
	} {
		t.Run(name, func(t *testing.T) {
			in := inputs(withTests("rust/ca/src/engine.rs", "go/pkga/a_test.go"))
			mutate(&in)
			p := BuildPlan(in)
			wantEverything(t, p, "")
			if len(p.Full) != 2 {
				t.Errorf("the full suite must still be planned: %+v", p.Full)
			}
		})
	}
}

func TestABlastFromAnotherHeadRunsEverything(t *testing.T) {
	in := inputs(withTests("rust/ca/src/engine.rs"))
	in.Blast.Change = baseSHA + ".." + strings.Repeat("3", 40)
	wantEverything(t, BuildPlan(in), "not the checked-out head")
	in.Blast.Change = "rust/ca/src/lib.rs:1-300"
	wantEverything(t, BuildPlan(in), "not the checked-out head")
}

func TestARustDcOrDevmapCliChangeSelectsTheGoPackagesThatDriveThoseBinaries(t *testing.T) {
	for _, file := range []string{"rust/dc-store/src/records.rs", "rust/devmap-cli/src/hook.rs"} {
		t.Run(file, func(t *testing.T) {
			// The graph reaches no Go test from here; that is the gap.
			in := inputs(withSeed(file, "f"))
			in.GitChanged = []string{file}
			p := BuildPlan(in)
			if p.Everything {
				t.Fatalf("widened to everything: %v", p.Widened)
			}
			got := cmds(p)
			want := []string{"go test -count=1 -json " + module + "/drv", "go test -count=1 -json " + module + "/viadevmap"}
			for _, w := range want {
				wantCmd(t, p, w)
			}
			for _, c := range got {
				if strings.Contains(c, "plain") || strings.Contains(c, "pkga") || strings.Contains(c, "testsupport") {
					t.Errorf("a package that drives no Rust binary was selected: %q", c)
				}
			}
			if len(p.Widened) != 1 || !strings.Contains(p.Widened[0].Reason, "blast-no-cross-process-rust-to-go-tests-2026-11-06") || !strings.Contains(p.Widened[0].Reason, file) {
				t.Errorf("widened = %+v", p.Widened)
			}
		})
	}
}

func TestACrateOutsideTheDriverRuleDoesNotSelectGoPackages(t *testing.T) {
	in := inputs(withTests("rust/ca/src/engine.rs"))
	in.GitChanged = []string{"rust/ca/src/engine.rs"}
	p := BuildPlan(in)
	wantCmd(t, p, "cargo test --locked -p ca --lib --no-fail-fast -- engine::")
	for _, c := range cmds(p) {
		if strings.HasPrefix(c, "go test") {
			t.Errorf("a non-driver crate selected Go: %q", c)
		}
	}
}

func TestTheFullSuiteIsTheWorkspaceLessTheExcludesAndEveryGoPackage(t *testing.T) {
	p := BuildPlan(inputs(withTests("rust/ca/src/engine.rs")))
	if len(p.Full) != 2 {
		t.Fatalf("full = %+v", p.Full)
	}
	want := "cargo test --locked --workspace --exclude dc-glob --exclude dc-proc --exclude dc-redact --exclude dc-grep --exclude dc-store --exclude dc-verify --exclude dc-evidence --no-fail-fast"
	if p.Full[0].Display != want || p.Full[0].Dir != "rust" || p.Full[0].Log == "" {
		t.Errorf("rust full = %+v", p.Full[0])
	}
	if p.Full[1].Display != "go test ./... -count=1 -json" || !p.Full[1].GoJSON || p.Full[1].Dir != "go" || p.Full[1].Log == "" {
		t.Errorf("go full = %+v", p.Full[1])
	}
}

// The real shapes. These are `devmap blast --since` outputs from this
// repository, trimmed, so a change of field name in the producer fails here
// and not as an empty selection.
func realBlast(t *testing.T, name string) *devmap.Blast {
	t.Helper()
	body, err := os.ReadFile(filepath.Join("testdata", name))
	if err != nil {
		t.Fatal(err)
	}
	b, err := devmap.DecodeBlast(body)
	if err != nil {
		t.Fatalf("%s: %v", name, err)
	}
	return b
}

func TestARealThreeCommitBlastWidensTheCratesItCouldNotPlaceAndDropsTheExcludedOne(t *testing.T) {
	b := realBlast(t, "blast-rust-three-commits.json")
	in := inputs()
	in.Blast = b
	in.Head = b.Head()
	in.Rust = &RustWorkspace{Packages: []RustPackage{
		{Name: "dc-grep", Dir: "rust/dc-grep", Targets: []RustTarget{{Name: "dc_grep", Kinds: []string{"lib"}, Src: "rust/dc-grep/src/lib.rs", Test: true}}},
		{Name: "devmap-cli", Dir: "rust/devmap-cli", Targets: []RustTarget{{Name: "devmap", Kinds: []string{"bin"}, Src: "rust/devmap-cli/src/main.rs", Test: true}}},
		{Name: "devmap-extract", Dir: "rust/devmap-extract", Targets: []RustTarget{{Name: "devmap_extract", Kinds: []string{"lib"}, Src: "rust/devmap-extract/src/lib.rs", Test: true}}},
	}}
	p := BuildPlan(in)
	if p.Everything {
		t.Fatalf("a real three-commit blast widened to everything: %v", p.Widened)
	}
	wantCmd(t, p, "cargo test --locked -p devmap-cli --no-fail-fast")
	wantCmd(t, p, "cargo test --locked -p devmap-extract --no-fail-fast")
	for _, c := range cmds(p) {
		if strings.Contains(c, "dc-grep") {
			t.Errorf("dc-grep is outside the full suite: %q", c)
		}
	}
	if len(p.Notices) == 0 || !strings.Contains(strings.Join(p.Notices, "\n"), "impact_incomplete (depth=3 reached=238)") {
		t.Errorf("the depth cap was not logged: %q", p.Notices)
	}
	if p.Blast == nil || p.Blast.SeedsTotal == 0 || len(p.Blast.Seeds) == 0 {
		t.Errorf("the seeds were not recorded: %+v", p.Blast)
	}
}

func TestARealGoBlastSelectsThePackagesItsTestFilesLiveIn(t *testing.T) {
	b := realBlast(t, "blast-go-policy.json")
	in := inputs()
	in.Blast = b
	in.Head = headSHA
	in.Blast.Change = baseSHA + ".." + headSHA
	in.GoDir = "backend/go_orchestrator"
	in.Go = &GoWorkspace{Module: module, Packages: []GoPackage{
		{ImportPath: module + "/cmd/devcouncil", Dir: "backend/go_orchestrator/cmd/devcouncil", TestFiles: []string{"backend/go_orchestrator/cmd/devcouncil/liveness_test.go"}},
		{ImportPath: module + "/policy", Dir: "backend/go_orchestrator/policy", TestFiles: []string{"backend/go_orchestrator/policy/command_bindings_test.go"}},
	}}
	p := BuildPlan(in)
	if p.Everything {
		t.Fatalf("widened: %v", p.Widened)
	}
	wantCmd(t, p, "go test -count=1 -json "+module+"/cmd/devcouncil")
	policy := wantCmd(t, p, "go test -count=1 -json "+module+"/policy")
	if len(policy.Seeds) == 0 || !strings.Contains(policy.Seeds[0], "backend/go_orchestrator/policy/command.go::") {
		t.Errorf("policy's seeds = %v", policy.Seeds)
	}
}

func TestEveryRealBlastDecodesAndTheUnindexedOneRunsEverything(t *testing.T) {
	for _, name := range []string{"blast-rust-three-commits.json", "blast-go-policy.json", "blast-dc-store.json", "blast-at-unindexed.json"} {
		b := realBlast(t, name)
		if len(b.Seeds) == 0 && name != "blast-at-unindexed.json" {
			t.Errorf("%s decoded to no seeds", name)
		}
	}
	b := realBlast(t, "blast-at-unindexed.json")
	in := inputs()
	in.Blast = b
	in.Blast.Change = baseSHA + ".." + headSHA
	wantEverything(t, BuildPlan(in), "not indexed")
}

func TestAChangedFileThatIsNeitherRustNorGoRunsEverything(t *testing.T) {
	for _, file := range []string{"scripts/helper.py", "scripts/helper.mjs", "rust/ca/tests/data/x.json", "docs/readme.md", "rust/.cargo/config.toml"} {
		t.Run(file, func(t *testing.T) {
			wantEverything(t, BuildPlan(inputs(withChanged(file), withTests("rust/ca/src/engine.rs"))), file+" changed with no graph edge to a test")
			in := inputs(withTests("rust/ca/src/engine.rs"))
			in.GitChanged = []string{file}
			wantEverything(t, BuildPlan(in), "no graph edge to a test")
		})
	}
}

func TestDoctestsAreSelectedWithTheLibraryUnlessThereAreNone(t *testing.T) {
	p := BuildPlan(inputs(withTests("rust/ca/src/engine.rs")))
	doc := wantCmd(t, p, "cargo test --locked -p ca --doc --no-fail-fast")
	if len(doc.Units) != 1 || !reflect.DeepEqual(doc.Units[0], Unit{Kind: "rust", Pkg: "ca", Target: "--doc"}) {
		t.Errorf("units = %+v", doc.Units)
	}
	// A whole-crate selection already includes the doctests.
	p = BuildPlan(inputs(withUnattributed("rust/ca/src/lib.rs", "outside_every_symbol")))
	for _, c := range cmds(p) {
		if strings.Contains(c, "--doc") {
			t.Errorf("a whole-crate command is followed by a redundant --doc: %q", c)
		}
	}
	// doctest = false, a bin-only crate, and a binary-only selection select none.
	in := inputs(withTests("rust/ca/src/engine.rs", "rust/devmap-cli/src/hook.rs"))
	for i := range in.Rust.Packages {
		if in.Rust.Packages[i].Name == "ca" {
			for j := range in.Rust.Packages[i].Targets {
				in.Rust.Packages[i].Targets[j].Doctest = false
			}
		}
	}
	for _, c := range cmds(BuildPlan(in)) {
		if strings.Contains(c, "--doc") {
			t.Errorf("doctest=false or bin-only crate got %q", c)
		}
	}
}

func TestSelectedCommandsLogToThePhasesLogsAndGoRunsAsJSON(t *testing.T) {
	p := BuildPlan(inputs(withTests("rust/ca/src/engine.rs", "go/pkga/a_test.go")))
	rust, golang := 0, 0
	for _, s := range p.Selected {
		switch {
		case s.Argv[0] == "go":
			golang++
			if !s.GoJSON || s.Log != "selected-go.jsonl" {
				t.Errorf("%q: GoJSON=%t log=%q", s.Display, s.GoJSON, s.Log)
			}
		default:
			rust++
			if s.Log != "selected-rust.log" {
				t.Errorf("%q log = %q", s.Display, s.Log)
			}
		}
	}
	if rust == 0 || golang == 0 {
		t.Fatalf("rust=%d go=%d: %q", rust, golang, cmds(p))
	}
}
