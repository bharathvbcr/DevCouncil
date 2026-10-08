//! Package-level function variables, closures, and platform-conditioned
//! targets: what the resolver does with each, reproduced before anything is
//! changed (`docs/devmap/BENCHMARK_HARDENING_AUDIT.md:78`).
//!
//! Each test pins the behaviour measured on 2026-10-08. Where that behaviour is
//! a limit rather than a correct answer, `docs/devmap/DIVERGENCES.md` ("Known
//! limits: function values and platform targets") records it under the test's
//! name, so the day a limit is lifted this file fails and the record has to
//! move with it.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction, SymbolKind};
use devmap_resolve::model::{Resolution, ResolutionResult};
use devmap_resolve::Resolver;

fn extract_all(files: &[(&str, &str)]) -> Vec<Extraction> {
    files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect()
}

fn resolve(files: &[(&str, &str)]) -> ResolutionResult {
    let extractions = extract_all(files);
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions).unwrap()
}

fn edges(result: &ResolutionResult, kind: EdgeKind) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = result
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == kind)
        .map(|edge| (edge.source_symbol.clone(), edge.target_symbol.clone()))
        .collect();
    found.sort();
    found
}

fn class_of(result: &ResolutionResult, from: &str, callee: &str) -> Vec<&'static str> {
    result
        .unresolved
        .iter()
        .filter(|row| row.source_symbol == from && row.callee_name == callee)
        .map(|row| row.class.label())
        .collect()
}

/// Known limit. An unexported Go package variable holding a function literal
/// is no symbol, so `handler()` names nothing the index holds (`no_namesake`)
/// and the literal's own calls belong to the file. `helper` keeps a caller —
/// the file — so it is not reported dead; what is lost is the path
/// `Run → handler → helper`.
#[test]
fn a_go_package_function_variable_is_not_a_symbol() {
    let result = resolve(&[(
        "app/app.go",
        "package app\n\nvar handler = func() int { return helper() }\n\n\
         func helper() int { return 1 }\n\nfunc Run() int { return handler() }\n",
    )]);
    assert_eq!(
        edges(&result, EdgeKind::Calls),
        vec![("app/app.go".to_string(), "app/app.go::helper".to_string())]
    );
    assert_eq!(class_of(&result, "app/app.go::Run", "handler"), vec!["no_namesake"]);
}

/// Known limit, same cause: `var hook = realImpl` keeps `realImpl` live through
/// a file-level reference, and `hook()` binds to nothing.
#[test]
fn a_go_function_variable_bound_to_a_function_references_it_from_the_file() {
    let result = resolve(&[(
        "app/app.go",
        "package app\n\nvar hook func() int = realImpl\n\n\
         func realImpl() int { return 1 }\n\nfunc Run() int { return hook() }\n",
    )]);
    assert_eq!(
        edges(&result, EdgeKind::References),
        vec![("app/app.go".to_string(), "app/app.go::realImpl".to_string())]
    );
    assert!(edges(&result, EdgeKind::Calls).is_empty());
    assert_eq!(class_of(&result, "app/app.go::Run", "hook"), vec!["no_namesake"]);
}

/// Resolved. A module-level arrow function is a `Function` that owns its
/// body's calls, and a call to it binds.
#[test]
fn a_javascript_module_arrow_is_a_function_both_ways() {
    let result = resolve(&[(
        "src/a.js",
        "const helper = () => 1;\nexport const handler = () => helper();\n\
         export function main() { return handler(); }\n",
    )]);
    assert_eq!(
        edges(&result, EdgeKind::Calls),
        vec![
            ("src/a.js::handler".to_string(), "src/a.js::helper".to_string()),
            ("src/a.js::main".to_string(), "src/a.js::handler".to_string()),
        ]
    );
}

/// Resolved. A JavaScript closure bound to a local is a nested `Function`.
#[test]
fn a_javascript_local_closure_is_a_nested_function() {
    let result = resolve(&[(
        "src/b.js",
        "function helper() { return 1; }\nexport function main() {\n  \
         const f = () => helper();\n  return f();\n}\n",
    )]);
    assert_eq!(
        edges(&result, EdgeKind::Calls),
        vec![
            ("src/b.js::main".to_string(), "src/b.js::main.f".to_string()),
            ("src/b.js::main.f".to_string(), "src/b.js::helper".to_string()),
        ]
    );
}

/// Partly a limit. A Rust closure's calls belong to the enclosing function,
/// which keeps `helper` live and the path intact; the call `f(1)` is a
/// `local_binding` — the closure is no symbol to bind it to.
#[test]
fn a_rust_closure_calls_through_its_enclosing_function() {
    let result = resolve(&[(
        "src/lib.rs",
        "fn helper(x: u32) -> u32 { x }\npub fn main() -> u32 {\n    \
         let f = |x| helper(x);\n    f(1)\n}\n",
    )]);
    assert_eq!(
        edges(&result, EdgeKind::Calls),
        vec![("src/lib.rs::main".to_string(), "src/lib.rs::helper".to_string())]
    );
    assert_eq!(class_of(&result, "src/lib.rs::main", "f"), vec!["local_binding"]);
}

/// Known limit. No platform is selected: both `open`s are real targets on
/// some platform, and the call fans out to each at the ambiguous tier. Both
/// stay live, which is the right answer for dead-code purposes; a build for
/// one platform would reach only one.
#[test]
fn go_platform_twins_fan_out_by_file_suffix_and_by_build_tag() {
    for (label, files) in [
        (
            "suffix",
            [
                ("sys/open_linux.go", "package sys\n\nfunc open() int { return 1 }\n"),
                ("sys/open_darwin.go", "package sys\n\nfunc open() int { return 2 }\n"),
                ("sys/use.go", "package sys\n\nfunc Use() int { return open() }\n"),
            ],
        ),
        (
            "build tag",
            [
                ("sys/open_linux.go", "//go:build linux\n\npackage sys\n\nfunc open() int { return 1 }\n"),
                ("sys/open_darwin.go", "//go:build !linux\n\npackage sys\n\nfunc open() int { return 2 }\n"),
                ("sys/use.go", "package sys\n\nfunc Use() int { return open() }\n"),
            ],
        ),
    ] {
        let result = resolve(&files);
        let calls: Vec<_> = result
            .edges
            .iter()
            .filter(|edge| edge.edge_kind == EdgeKind::Calls && edge.source_symbol == "sys/use.go::Use")
            .collect();
        let mut targets: Vec<&str> = calls.iter().map(|edge| edge.target_symbol.as_str()).collect();
        targets.sort();
        assert_eq!(
            targets,
            vec!["sys/open_darwin.go::open", "sys/open_linux.go::open"],
            "{label}"
        );
        assert!(
            calls.iter().all(|edge| matches!(
                edge.resolution.as_deref(),
                Some(Resolution::AmbiguousGlobal { .. })
            )),
            "{label}: a platform twin is a guess between candidates, never a single bind"
        );
    }
}

/// Known limit. `#[cfg(unix)]` and `#[cfg(windows)]` twins in one file get one
/// qualified name, so they are one node identity and the call's single edge
/// cannot say which it reached.
#[test]
fn rust_cfg_twins_share_one_identity() {
    let source = "#[cfg(unix)]\nfn open() -> u32 { 1 }\n#[cfg(windows)]\nfn open() -> u32 { 2 }\n\
                  pub fn run() -> u32 { open() }\n";
    let extraction = extract_file("src/lib.rs", source);
    let twins: Vec<&str> = extraction
        .symbols
        .iter()
        .filter(|symbol| symbol.kind == SymbolKind::Function && symbol.name == "open")
        .map(|symbol| symbol.qualified_name.as_str())
        .collect();
    assert_eq!(twins, vec!["src/lib.rs::open", "src/lib.rs::open"]);
    let result = resolve(&[("src/lib.rs", source)]);
    assert_eq!(
        edges(&result, EdgeKind::Calls),
        vec![("src/lib.rs::run".to_string(), "src/lib.rs::open".to_string())]
    );
}
