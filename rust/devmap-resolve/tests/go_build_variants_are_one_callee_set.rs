//! A Go call names the *set* of build-constraint variants of a function.
//!
//! `proc.ConfigureGroup` is declared once under `//go:build unix` and once
//! under `//go:build !unix`; exactly one of them is in any build, and a call
//! reaches whichever one compiled. The resolver saw two declarations, could not
//! pick, and left the call in the ledger as `AmbiguousGlobal` (a bare call in
//! the package), `uninferred_receiver` (a package-qualified call from another
//! package) or, worse, bound it to an unrelated function of the same name in a
//! different package. Measured against `go/types` on this repository that was
//! about 74 of the 84 Go calls DevMap missed.
//!
//! The rule: same directory, same package clause, same function name, two or
//! more declaring files, **every** one of them build-constrained (Go forbids
//! two declarations of one name in a package, so constrained twins are
//! mutually exclusive in any build that compiles). One edge per variant, at the
//! dispatch tier's confidence, with provenance in the edge details. Never across
//! packages.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::model::{Resolution, ResolutionKind, ResolutionResult, ResolvedEdge};
use devmap_resolve::Resolver;

const GOMOD: &str = "module example.com/app\n\ngo 1.22\n";

const UNIX: &str = "//go:build unix\n\npackage proc\n\nfunc ConfigureGroup() {}\n";
const OTHER: &str = "//go:build !unix\n\npackage proc\n\nfunc ConfigureGroup() {}\n";
const SAMEPKG_CALLER: &str = "package proc\n\nfunc Command() { ConfigureGroup() }\n";
const OTHERPKG_CALLER: &str =
    "package components\n\nimport \"example.com/app/proc\"\n\nfunc Build() { proc.ConfigureGroup() }\n";
/// An unrelated package that also declares `ConfigureGroup`, once.
const NAMESAKE: &str = "package elsewhere\n\nfunc ConfigureGroup() {}\n";

fn resolve(files: &[(&str, &str)]) -> ResolutionResult {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    let module = devmap_extract::parse_go_mod("go.mod", GOMOD).expect("go.mod parses");
    resolver.index_go_modules(std::slice::from_ref(&module));
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions).unwrap()
}

fn calls_from<'a>(result: &'a ResolutionResult, caller: &str) -> Vec<&'a ResolvedEdge> {
    result
        .edges
        .iter()
        .filter(|edge| {
            edge.edge_kind == EdgeKind::Calls
                && edge.source_symbol.rsplit("::").next() == Some(caller)
        })
        .collect()
}

fn targets(edges: &[&ResolvedEdge]) -> Vec<String> {
    let mut targets: Vec<String> = edges.iter().map(|edge| edge.target_file.clone()).collect();
    targets.sort();
    targets
}

#[test]
fn a_bare_call_in_the_package_reaches_every_variant() {
    let result = resolve(&[
        ("proc/procgroup_unix.go", UNIX),
        ("proc/procgroup_other.go", OTHER),
        ("proc/git.go", SAMEPKG_CALLER),
    ]);
    let edges = calls_from(&result, "Command");
    assert_eq!(
        targets(&edges),
        ["proc/procgroup_other.go", "proc/procgroup_unix.go"],
        "one edge per build variant: {edges:?}"
    );
    for edge in &edges {
        assert_eq!(
            edge.resolution.as_ref().unwrap().kind(),
            ResolutionKind::Dispatch
        );
        assert_eq!(edge.confidence, ResolutionKind::Dispatch.confidence());
        let details = edge.details.as_deref().unwrap_or("");
        assert!(
            details.contains("build variants") && details.contains("ConfigureGroup"),
            "the edge must say why it fans out: {details:?}"
        );
    }
    assert!(
        !result
            .unresolved
            .iter()
            .any(|row| row.callee_name == "ConfigureGroup"),
        "the call is resolved, not ledgered: {:?}",
        result.unresolved
    );
}

#[test]
fn a_package_qualified_call_from_another_package_reaches_every_variant() {
    let result = resolve(&[
        ("proc/procgroup_unix.go", UNIX),
        ("proc/procgroup_other.go", OTHER),
        ("components/build.go", OTHERPKG_CALLER),
    ]);
    let edges = calls_from(&result, "Build");
    assert_eq!(
        targets(&edges),
        ["proc/procgroup_other.go", "proc/procgroup_unix.go"],
        "{edges:?}"
    );
}

/// The qualified call is the one the unrelated namesake used to capture.
#[test]
fn a_namesake_in_another_package_is_never_bound() {
    let result = resolve(&[
        ("proc/procgroup_unix.go", UNIX),
        ("proc/procgroup_other.go", OTHER),
        ("elsewhere/e.go", NAMESAKE),
        ("components/build.go", OTHERPKG_CALLER),
        ("proc/git.go", SAMEPKG_CALLER),
    ]);
    for caller in ["Build", "Command"] {
        for edge in calls_from(&result, caller) {
            assert!(
                edge.target_file.starts_with("proc/"),
                "{caller} bound across packages: {edge:?}"
            );
        }
    }
}

/// Two unconstrained declarations are not variants — that is a package that
/// does not compile, or an extraction fault — so the call stays ambiguous.
#[test]
fn unconstrained_twins_are_not_variants() {
    let twin = "package proc\n\nfunc ConfigureGroup() {}\n";
    let result = resolve(&[
        ("proc/a.go", twin),
        ("proc/b.go", twin),
        ("proc/git.go", SAMEPKG_CALLER),
    ]);
    assert!(
        calls_from(&result, "Command").is_empty(),
        "{:?}",
        calls_from(&result, "Command")
    );
}

/// One constrained declaration beside an unconstrained one is not a variant
/// set either: the unconstrained file is in every build.
#[test]
fn a_constrained_file_beside_an_unconstrained_one_is_not_a_variant_set() {
    let plain = "package proc\n\nfunc ConfigureGroup() {}\n";
    let result = resolve(&[
        ("proc/procgroup_unix.go", UNIX),
        ("proc/plain.go", plain),
        ("proc/git.go", SAMEPKG_CALLER),
    ]);
    assert!(
        calls_from(&result, "Command").is_empty(),
        "{:?}",
        calls_from(&result, "Command")
    );
}

/// A single declaration is the ordinary rung, untouched.
#[test]
fn one_constrained_declaration_is_the_ordinary_same_package_edge() {
    let result = resolve(&[
        ("proc/procgroup_unix.go", UNIX),
        ("proc/git.go", SAMEPKG_CALLER),
    ]);
    let edges = calls_from(&result, "Command");
    assert_eq!(targets(&edges), ["proc/procgroup_unix.go"]);
    assert!(matches!(
        edges[0].resolution.as_deref(),
        Some(Resolution::SamePackage { .. })
    ));
}

/// A filename suffix is a constraint too (`_windows.go`, `_linux.go`) with no
/// `//go:build` line at all.
#[test]
fn filename_suffix_variants_count() {
    let win = "package term\n\nfunc width() int { return 80 }\n";
    let lin = "package term\n\nfunc width() int { return 120 }\n";
    let caller = "package term\n\nfunc Render() int { return width() }\n";
    let result = resolve(&[
        ("term/width_windows.go", win),
        ("term/width_linux.go", lin),
        ("term/render.go", caller),
    ]);
    assert_eq!(
        targets(&calls_from(&result, "Render")),
        ["term/width_linux.go", "term/width_windows.go"]
    );
}

/// An external test package (`package proc_test`) is another package even in
/// the same directory: the variants of `proc` are not its package block.
#[test]
fn a_call_from_the_external_test_package_does_not_see_the_package_block() {
    let xtest = "package proc_test\n\nfunc TestX() { ConfigureGroup() }\n";
    let result = resolve(&[
        ("proc/procgroup_unix.go", UNIX),
        ("proc/procgroup_other.go", OTHER),
        ("proc/x_test.go", xtest),
    ]);
    assert!(
        calls_from(&result, "TestX").is_empty(),
        "{:?}",
        calls_from(&result, "TestX")
    );
}

/// A method is not a package-level name: two constrained methods of one name
/// are decided by the receiver's type, which the call does not carry.
#[test]
fn constrained_methods_are_not_a_variant_set() {
    let unix = "//go:build unix\n\npackage proc\n\ntype S struct{}\n\nfunc (S) Close() {}\n";
    let other = "//go:build !unix\n\npackage proc\n\nfunc (S) Close() {}\n";
    let caller = "package proc\n\nfunc Run(s S) { Close() }\n";
    let result = resolve(&[
        ("proc/a_unix.go", unix),
        ("proc/a_other.go", other),
        ("proc/run.go", caller),
    ]);
    assert!(calls_from(&result, "Run").is_empty());
}

/// Past the dispatch fan-out cap the call is ledgered with the whole candidate
/// list instead of fanning out — the same bound every other candidate set has.
#[test]
fn a_variant_set_past_the_fanout_cap_is_ledgered_not_fanned_out() {
    let mut files: Vec<(String, String)> = (0..17)
        .map(|n| {
            (
                format!("proc/v{n}.go"),
                format!("//go:build tag{n}\n\npackage proc\n\nfunc ConfigureGroup() {{}}\n"),
            )
        })
        .collect();
    files.push(("proc/git.go".into(), SAMEPKG_CALLER.into()));
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, s)| (p.as_str(), s.as_str()))
        .collect();
    let result = resolve(&refs);
    assert!(calls_from(&result, "Command").is_empty());
    assert!(
        result
            .unresolved
            .iter()
            .any(|row| row.callee_name == "ConfigureGroup"),
        "the site must stay in the ledger"
    );
}

/// Variants declared only in `_test.go` files are in no production build: a
/// production caller must not reach them, a test caller reaches both.
#[test]
fn test_file_variants_are_visible_to_tests_only() {
    let unix = "//go:build unix\n\npackage proc\n\nfunc lockAll() {}\n";
    let other = "//go:build !unix\n\npackage proc\n\nfunc lockAll() {}\n";
    let prod = "package proc\n\nfunc Run() { lockAll() }\n";
    let test = "package proc\n\nfunc TestRun() { lockAll() }\n";
    let result = resolve(&[
        ("proc/lock_unix_test.go", unix),
        ("proc/lock_other_test.go", other),
        ("proc/run.go", prod),
        ("proc/run_test.go", test),
    ]);
    assert!(
        calls_from(&result, "Run").is_empty(),
        "{:?}",
        calls_from(&result, "Run")
    );
    assert_eq!(
        targets(&calls_from(&result, "TestRun")),
        ["proc/lock_other_test.go", "proc/lock_unix_test.go"]
    );
}

/// A call from inside a variant file names its own variant (the same-file rung
/// runs first) — it is not widened to the set.
#[test]
fn a_call_from_inside_a_variant_stays_on_its_own_variant() {
    let unix = "//go:build unix\n\npackage proc\n\nfunc ConfigureGroup() {}\n\nfunc Own() { ConfigureGroup() }\n";
    let result = resolve(&[
        ("proc/procgroup_unix.go", unix),
        ("proc/procgroup_other.go", OTHER),
    ]);
    assert_eq!(
        targets(&calls_from(&result, "Own")),
        ["proc/procgroup_unix.go"]
    );
}
