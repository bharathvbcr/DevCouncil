//! ScholarLM-scale findings: tests that fail against the pre-fix kernel.
//!
//! Each test names the invariant the production code must hold. They are
//! written against public APIs so a regression cannot hide behind a helper
//! that the CLI never calls.

use devmap_analyze::*;
use devmap_extract::*;
use devmap_query::*;
use devmap_resolve::*;
use devmap_store::*;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

fn resolve(files: &[Extraction]) -> ResolutionResult {
    let mut resolver = Resolver::new();
    resolver.index_extractions(files);
    resolver.resolve_all(files)
}

fn resolve_with_modules(files: &[Extraction], modules: &[GoModule]) -> ResolutionResult {
    let mut resolver = Resolver::new();
    resolver.index_go_modules(modules);
    resolver.index_extractions(files);
    resolver.resolve_all(files)
}

fn temp_dir(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("devmap-{label}-{stamp}"));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

fn init_git(root: &Path) {
    let status = std::process::Command::new("git")
        .args(["init"])
        .current_dir(root)
        .status()
        .expect("git init");
    assert!(status.success(), "git init failed");
}

fn confident_dead<'a>(analysis: &'a AnalysisSummary, name: &str) -> Option<&'a DeadSymbolReport> {
    analysis.dead_symbols.iter().find(|report| {
        !report.is_exempt
            && report.confidence >= 0.9
            && (report.symbol_name == name
                || report.symbol_name.ends_with(&format!(".{name}"))
                || report.symbol_name.ends_with(&format!("::{name}")))
    })
}

// ---------------------------------------------------------------------------
// 1. Go import resolution
// ---------------------------------------------------------------------------

#[test]
fn go_module_prefix_resolves_internal_imports_and_ignores_stdlib() {
    let importer = extract_file(
        "internal/foo/a.go",
        r#"package foo

import (
    "fmt"
    "scholarlm/backend/go_orchestrator/internal/bar"
)

func Use() { bar.Helper() }
"#,
    );
    let target = extract_file("internal/bar/b.go", "package bar\n\nfunc Helper() {}\n");
    let other = extract_file("internal/bar/c.go", "package bar\n\nfunc Extra() {}\n");
    let files = [importer, target, other];
    let modules = [GoModule {
        prefix: "scholarlm/backend/go_orchestrator".into(),
        dir: String::new(),
        replaces: vec![],
    }];
    let result = resolve_with_modules(&files, &modules);

    let imports: Vec<_> = result
        .edges
        .iter()
        .filter(|edge| {
            edge.edge_kind == EdgeKind::Imports && edge.source_file == "internal/foo/a.go"
        })
        .collect();
    assert!(
        imports
            .iter()
            .any(|edge| edge.target_file == "package:internal/bar/bar"),
        "module-prefix import must resolve to the package node, got {imports:?}"
    );
    assert_eq!(
        imports.len(),
        1,
        "one import spec must not fan out to every package file: {imports:?}"
    );
    assert!(
        result.edges.iter().any(|edge| {
            edge.edge_kind == EdgeKind::MemberOf && edge.source_file == "internal/bar/c.go"
        }),
        "Go packages are directories: Extra in c.go must MemberOf the imported package"
    );
    assert!(
        result.edges.iter().any(|edge| {
            edge.edge_kind == EdgeKind::Calls && edge.target_symbol.contains("Helper")
        }),
        "bar.Helper must still resolve through the package import: {:?}",
        result.edges
    );
    assert!(
        !imports.iter().any(|edge| edge.target_file.contains("fmt")),
        "stdlib fmt must not fabricate a file edge: {imports:?}"
    );
}

#[test]
fn go_import_suffix_match_works_without_go_mod() {
    let importer = extract_file(
        "pkg/client/client.go",
        "package client\nimport \"example.com/mod/pkg/api\"\nfunc Call() {}\n",
    );
    let target = extract_file("pkg/api/api.go", "package api\nfunc Get() {}\n");
    let result = resolve(&[importer, target]);
    assert!(
        result.edges.iter().any(|edge| {
            edge.edge_kind == EdgeKind::Imports
                && edge.source_file == "pkg/client/client.go"
                && edge.target_file == "package:pkg/api/api"
        }),
        "unique path-boundary suffix of the import spec must resolve without go.mod: {:?}",
        result.edges
    );
}

#[test]
fn go_import_does_not_guess_ambiguous_suffixes() {
    let importer = extract_file(
        "svc/main.go",
        "package main\nimport \"example.com/mod/foo\"\n",
    );
    let first = extract_file("a/foo/a.go", "package foo\n");
    let second = extract_file("b/foo/b.go", "package foo\n");
    let result = resolve(&[importer, first, second]);
    assert!(
        !result.edges.iter().any(|edge| {
            edge.edge_kind == EdgeKind::Imports && edge.source_file == "svc/main.go"
        }),
        "two equally-long suffix matches must not pick a winner: {:?}",
        result.edges
    );
}

#[test]
fn go_replace_directive_retargets_an_external_module() {
    let importer = extract_file(
        "cmd/main.go",
        "package main\nimport \"github.com/acme/lib\"\n",
    );
    let replaced = extract_file("third_party/lib/lib.go", "package lib\nfunc X() {}\n");
    let modules = [GoModule {
        prefix: "example.com/app".into(),
        dir: String::new(),
        replaces: vec![("github.com/acme/lib".into(), "third_party/lib".into())],
    }];
    let result = resolve_with_modules(&[importer, replaced], &modules);
    assert!(
        result.edges.iter().any(|edge| {
            edge.edge_kind == EdgeKind::Imports
                && edge.source_file == "cmd/main.go"
                && edge.target_file == "package:third_party/lib/lib"
        }),
        "replace must retarget the import: {:?}",
        result.edges
    );
}

#[test]
fn go_aliased_import_binds_the_alias_not_the_path_tail() {
    let importer = extract_file(
        "pkg/a.go",
        "package a\nimport w \"example.com/mod/pkg/b\"\nfunc Use() { w.Helper() }\n",
    );
    let target = extract_file("pkg/b.go", "package b\nfunc Helper() {}\n");
    let result = resolve(&[importer, target]);
    assert!(
        result.edges.iter().any(|edge| {
            edge.edge_kind == EdgeKind::Calls && edge.target_symbol.ends_with("Helper")
        }),
        "aliased import must resolve w.Helper: {:?}",
        result.edges
    );
}

#[test]
fn collect_go_modules_reads_module_and_replace_lines() {
    let root = temp_dir("gomod");
    fs::write(
        root.join("go.mod"),
        "module scholarlm/backend/go_orchestrator\n\nreplace github.com/acme/lib => ../lib\n",
    )
    .unwrap();
    let modules = collect_go_modules(&root).unwrap();
    assert_eq!(modules.len(), 1);
    assert_eq!(modules[0].prefix, "scholarlm/backend/go_orchestrator");
    assert_eq!(
        modules[0].replaces,
        vec![("github.com/acme/lib".into(), "../lib".into())]
    );
    let _ = fs::remove_dir_all(root);
}

// ---------------------------------------------------------------------------
// 2. Go receiver methods + same-file liveness
// ---------------------------------------------------------------------------

#[test]
fn go_method_receiver_is_the_declared_type_not_a_constructor_guess() {
    let source = extract_file(
        "eval.go",
        r#"package eval

type HallucinationsEvaluator struct{}

func (e *HallucinationsEvaluator) segment() {}

func (e *HallucinationsEvaluator) evaluateInvocation() {
    e.segment()
}
"#,
    );
    assert!(
        source.symbols.iter().any(|symbol| {
            symbol.name == "segment"
                && symbol.kind == SymbolKind::Method
                && symbol.qualified_name == "eval.go::HallucinationsEvaluator.segment"
                && symbol.parent_symbol.as_deref() == Some("eval.go::HallucinationsEvaluator")
        }),
        "Go method identity must be file::Type.name, got {:?}",
        source.symbols
    );
    let result = resolve(std::slice::from_ref(&source));
    assert!(
        result.edges.iter().any(|edge| {
            edge.edge_kind == EdgeKind::Calls
                && edge
                    .target_symbol
                    .ends_with("HallucinationsEvaluator.segment")
        }),
        "e.segment() must resolve via the declared receiver: {:?}",
        result.edges
    );
    let analysis = analyze(std::slice::from_ref(&source), &result);
    assert!(
        confident_dead(&analysis, "segment").is_none(),
        "same-file receiver call must not be confident-dead: {:?}",
        analysis.dead_symbols
    );
}

#[test]
fn go_value_and_pointer_receivers_both_type_the_binding() {
    let source = extract_file(
        "client.go",
        r#"package client

type paperclipClient struct{}

func (c paperclipClient) headers() {}

func (c *paperclipClient) send() {
    c.headers()
}
"#,
    );
    let result = resolve(std::slice::from_ref(&source));
    let analysis = analyze(std::slice::from_ref(&source), &result);
    assert!(
        confident_dead(&analysis, "headers").is_none(),
        "value-receiver method called via pointer receiver must stay live: {:?}",
        analysis.dead_symbols
    );
}

#[test]
fn go_struct_used_only_in_type_position_is_not_dead() {
    let source = extract_file(
        "types.go",
        r#"package types

type failureRecord struct{ Msg string }

func Handle(rec failureRecord) failureRecord { return rec }
"#,
    );
    let result = resolve(std::slice::from_ref(&source));
    let analysis = analyze(std::slice::from_ref(&source), &result);
    assert!(
        result.edges.iter().any(|edge| {
            edge.edge_kind == EdgeKind::References && edge.target_symbol.ends_with("failureRecord")
        }),
        "type-position uses must emit References edges: {:?}",
        result.edges
    );
    assert!(
        confident_dead(&analysis, "failureRecord").is_none(),
        "unexported struct used only as a type must not be confident-dead: {:?}",
        analysis.dead_symbols
    );
}

// ---------------------------------------------------------------------------
// 3. Discarded references (JSX / type / shorthand)
// ---------------------------------------------------------------------------

#[test]
fn jsx_identifier_prop_keeps_the_handler_live() {
    let source = extract_file(
        "Modal.tsx",
        r#"export function Modal() {
  const handleClose = () => {};
  return <button onClick={handleClose}>x</button>;
}
"#,
    );
    assert!(
        source.references.iter().any(|reference| {
            reference.name == "handleClose" && reference.kind == ReferenceKind::Name
        }),
        "onClick={{handleClose}} must extract a Name reference, got {:?}",
        source.references
    );
    let result = resolve(std::slice::from_ref(&source));
    let analysis = analyze(std::slice::from_ref(&source), &result);
    assert!(
        confident_dead(&analysis, "handleClose").is_none(),
        "JSX identifier use must keep handleClose live: {:?}",
        analysis.dead_symbols
    );
}

#[test]
fn typescript_type_annotation_is_a_reference_use() {
    let source = extract_file(
        "fail.ts",
        r#"type failureRecord = { msg: string };
export function report(rec: failureRecord): failureRecord { return rec; }
"#,
    );
    let result = resolve(std::slice::from_ref(&source));
    let analysis = analyze(std::slice::from_ref(&source), &result);
    assert!(
        confident_dead(&analysis, "failureRecord").is_none(),
        "type-annotation uses must keep failureRecord live: {:?}",
        analysis.dead_symbols
    );
}

#[test]
fn object_shorthand_keeps_the_named_function_live() {
    let source = extract_file(
        "ext.ts",
        r#"function parseHTML() {}
export const extensions = { parseHTML };
"#,
    );
    let result = resolve(std::slice::from_ref(&source));
    let analysis = analyze(std::slice::from_ref(&source), &result);
    assert!(
        confident_dead(&analysis, "parseHTML").is_none(),
        "object-shorthand {{ parseHTML }} is a use: {:?}",
        analysis.dead_symbols
    );
}

#[test]
fn structural_contains_edges_still_do_not_count_as_liveness() {
    let source = extract_file("alone.py", "def unused():\n    pass\n");
    let result = resolve(std::slice::from_ref(&source));
    assert!(result
        .edges
        .iter()
        .any(|edge| edge.edge_kind == EdgeKind::Contains));
    let analysis = analyze(std::slice::from_ref(&source), &result);
    assert!(
        confident_dead(&analysis, "unused").is_some(),
        "Contains must not launder an unused function into liveness: {:?}",
        analysis.dead_symbols
    );
}

// ---------------------------------------------------------------------------
// 4. Exact impact / trace matching
// ---------------------------------------------------------------------------

#[test]
fn impact_does_not_substring_match_every_go_file() {
    let files = vec![
        extract_file("alpha.go", "package a\nfunc Alpha() {}\n"),
        extract_file("beta.go", "package b\nfunc Beta() {}\n"),
        extract_file("gamma.go", "package g\nfunc Gamma() {}\n"),
    ];
    let result = resolve(&files);
    let engine = QueryEngine::new(&files, &result);
    let response = engine.impact(Request {
        query: "go".into(),
        token_budget: 2000,
        min_confidence: 0.0,
        max_depth: 3,
    });
    assert!(
        matches!(
            response.resolution,
            ResolutionAvailability::Unavailable { .. }
        ),
        "'go' is not a symbol or path; saturating on substring is a false Available: {:?}",
        response.resolution
    );
    assert!(response.items.is_empty());
}

#[test]
fn impact_single_letter_does_not_saturate_the_graph() {
    let files: Vec<_> = (0..20)
        .map(|i| extract_file(&format!("f{i}.py"), &format!("def e{i}():\n    pass\n")))
        .collect();
    let result = resolve(&files);
    let engine = QueryEngine::new(&files, &result);
    let response = engine.impact(Request {
        query: "e".into(),
        token_budget: 2000,
        min_confidence: 0.0,
        max_depth: 3,
    });
    assert!(
        matches!(
            response.resolution,
            ResolutionAvailability::Unavailable { .. }
        ) || response.total < 5,
        "single-letter substring must not report Available with a saturated walk: total={}",
        response.total
    );
}

#[test]
fn impact_qualified_symbol_does_not_match_unrelated_names() {
    let files = vec![
        extract_file(
            "hallucinations.go",
            "package h\nfunc segment() {}\nfunc segmentationCoverage() { segment() }\n",
        ),
        extract_file("other.go", "package o\nfunc segmenterResponse() {}\n"),
    ];
    let result = resolve(&files);
    let engine = QueryEngine::new(&files, &result);
    let response = engine.impact(Request {
        query: "hallucinations.go::segment".into(),
        token_budget: 2000,
        min_confidence: 0.0,
        max_depth: 3,
    });
    assert!(
        matches!(response.resolution, ResolutionAvailability::Available),
        "qualified identity must be found: {:?}",
        response.resolution
    );
    assert!(
        response.items.iter().all(|edge| {
            edge.source_symbol.contains("segment") || edge.target_symbol.contains("segment")
        }),
        "unrelated segmenterResponse must not appear: {:?}",
        response.items
    );
    assert!(
        !response.items.iter().any(|edge| {
            edge.source_symbol.contains("segmenterResponse")
                || edge.target_symbol.contains("segmenterResponse")
        }),
        "substring 'segment' inside another symbol is not an exact match: {:?}",
        response.items
    );
}

#[test]
fn impact_exact_symbol_name_still_works_for_short_unique_names() {
    let files = vec![
        extract_file("a.py", "def a(): pass\n"),
        extract_file("b.py", "def b(): pass\n"),
        extract_file("c.py", "def c(): pass\n"),
    ];
    let edge = |source: &str, target: &str| ResolvedEdge {
        source_file: format!("{source}.py"),
        target_file: format!("{target}.py"),
        source_symbol: source.to_string(),
        target_symbol: target.to_string(),
        edge_kind: EdgeKind::Calls,
        confidence: Confidence::DETERMINISTIC,
        resolution: None,
        details: None,
    };
    let resolution = ResolutionResult {
        edges: vec![edge("a", "b"), edge("b", "c")],
        receiver_types: Default::default(),
        reexport_chains: Default::default(),
        unresolved: Vec::new(),
    };
    let engine = QueryEngine::new(&files, &resolution);
    let depth_two = engine.impact(Request {
        query: "c".into(),
        token_budget: 2000,
        min_confidence: 0.0,
        max_depth: 2,
    });
    assert_eq!(depth_two.items.len(), 2);
}

// ---------------------------------------------------------------------------
// 5. Cold build vs watcher gitignore
// ---------------------------------------------------------------------------

#[test]
fn watcher_gitignore_honours_parent_rules_the_walker_sees() {
    let git_root = temp_dir("ignore-git");
    init_git(&git_root);
    fs::write(git_root.join(".gitignore"), "generated.py\n").unwrap();
    let sub = git_root.join("sub");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("kept.py"), "def kept():\n    return 1\n").unwrap();
    fs::write(sub.join("generated.py"), "def generated():\n    return 1\n").unwrap();

    let collected: BTreeSet<_> = collect_sources(&sub)
        .unwrap()
        .into_iter()
        .map(|(path, _)| path)
        .collect();
    assert_eq!(
        collected,
        ["kept.py".to_string()].into_iter().collect(),
        "WalkBuilder must exclude parent-gitignored generated.py: {collected:?}"
    );

    assert!(
        is_gitignored(&sub, &sub.join("generated.py"), false).unwrap(),
        "watcher ignore must agree with the cold walker on parent .gitignore"
    );
    assert!(
        !is_gitignored(&sub, &sub.join("kept.py"), false).unwrap(),
        "kept.py is not ignored"
    );

    let _ = fs::remove_dir_all(git_root);
}

#[test]
fn gitignore_negation_in_parent_is_honoured() {
    let git_root = temp_dir("ignore-neg");
    init_git(&git_root);
    fs::write(git_root.join(".gitignore"), "*.py\n!kept.py\n").unwrap();
    let sub = git_root.join("sub");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("kept.py"), "x = 1\n").unwrap();
    fs::write(sub.join("drop.py"), "x = 1\n").unwrap();

    assert!(is_gitignored(&sub, &sub.join("drop.py"), false).unwrap());
    assert!(!is_gitignored(&sub, &sub.join("kept.py"), false).unwrap());
    let collected: BTreeSet<_> = collect_sources(&sub)
        .unwrap()
        .into_iter()
        .map(|(path, _)| path)
        .collect();
    assert_eq!(collected, ["kept.py".to_string()].into_iter().collect());
    let _ = fs::remove_dir_all(git_root);
}

// ---------------------------------------------------------------------------
// 6. Manifest safety + consumer schema
// ---------------------------------------------------------------------------

#[test]
fn manifest_emits_consumer_keys_agents_are_told_to_read() {
    let files = vec![
        extract_file("src/main.py", "def main():\n    pass\n"),
        extract_file("src/util.py", "def helper():\n    pass\n"),
    ];
    let result = resolve(&files);
    let analysis = analyze(&files, &result);
    let (_manifest, json) = generate_manifest_with_edges(
        &files,
        &analysis,
        FreshnessInfo {
            head_sha: "abc".into(),
            generation_id: 1,
            pending_count: 0,
        },
        &result.edges,
    );
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    for key in [
        "files",
        "dependents",
        "dead_symbol_candidates",
        "liveness_meta",
        "entry_roots",
        "subsystems",
        "languages",
        "map_engine",
    ] {
        assert!(
            value.get(key).is_some(),
            "missing consumer key {key}: {json}"
        );
    }
    let listed: Vec<_> = value["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["path"].as_str().unwrap().to_string())
        .collect();
    assert!(listed.iter().any(|path| path == "src/main.py"));
    assert_eq!(value["map_engine"], "devmap-rust");
}

#[test]
fn manifest_refuses_to_overwrite_a_python_schema_map() {
    let root = temp_dir("manifest-guard");
    let map_path = root.join(".devcouncil/repo_map.json");
    fs::create_dir_all(map_path.parent().unwrap()).unwrap();
    fs::write(
        &map_path,
        r#"{"languages":[],"frameworks":[],"package_managers":[],"test_commands":[],"important_files":[],"candidate_files":[],"files":[{"path":"a.py","area":"a","kind":"code","summary":"x"}],"subsystems":[],"generated_head":"python"}"#,
    )
    .unwrap();
    let err = write_manifest_atomically(&map_path, "{}", false)
        .expect_err("foreign Python map must not be overwritten without --force");
    assert!(
        err.to_string().contains("refuse") || err.to_string().contains("force"),
        "error must mention the guard: {err}"
    );
    let kept = fs::read_to_string(&map_path).unwrap();
    assert!(kept.contains("generated_head"));
    write_manifest_atomically(&map_path, r#"{"map_engine":"devmap-rust"}"#, true).unwrap();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn default_manifest_path_is_the_indexed_repo_root_not_cwd() {
    let resolved =
        resolve_manifest_output(Some("/abs/repo"), Path::new(".devcouncil/repo_map.json"));
    assert!(
        resolved.ends_with("repo/.devcouncil/repo_map.json")
            || resolved == Path::new("/abs/repo/.devcouncil/repo_map.json"),
        "relative default must join the generation repo root, got {resolved:?}"
    );
}

// ---------------------------------------------------------------------------
// 7. Tree-sitter TypeScript / TSX
// ---------------------------------------------------------------------------

#[test]
fn tsx_bare_ampersand_in_jsx_text_parses_clean() {
    let ext = extract_file(
        "amp.tsx",
        "export function Amp() { return <div>Account & Settings</div>; }\n",
    );
    assert!(
        matches!(ext.parse_outcome, ParseOutcome::Clean),
        "bare & in JSX text is valid TSX, got {:?}",
        ext.parse_outcome
    );
    assert!(ext.symbols.iter().any(|symbol| symbol.name == "Amp"));
}

#[test]
fn tsx_ampersand_in_jsx_attribute_string_parses_clean() {
    let ext = extract_file(
        "attr.tsx",
        "export function Attr() { return <div className='a & b'>x</div>; }\n",
    );
    assert!(
        matches!(ext.parse_outcome, ParseOutcome::Clean),
        "bare & in a JSX attribute string is valid TSX, got {:?}",
        ext.parse_outcome
    );
}

#[test]
fn typescript_generic_importtype_does_not_swallow_the_export() {
    let ext = extract_file(
        "generic.ts",
        "export function importOriginal<T>() { return null as T; }\nexport const kept = 1;\n",
    );
    assert!(
        ext.symbols
            .iter()
            .any(|symbol| symbol.name == "importOriginal")
            || ext.symbols.iter().any(|symbol| symbol.name == "kept"),
        "a generic signature must not erase remaining exports: {:?}",
        ext.symbols
    );
}

// ---------------------------------------------------------------------------
// 8. Confidence milliconfidence
// ---------------------------------------------------------------------------

#[test]
fn persisted_high_confidence_survives_sql_inequality() {
    let ext = extract_file("dead.py", "def unused():\n    pass\n");
    let result = resolve(std::slice::from_ref(&ext));
    let analysis = analyze(std::slice::from_ref(&ext), &result);
    assert!(
        analysis
            .dead_symbols
            .iter()
            .any(|report| report.symbol_name == "unused" && (report.confidence - 0.9).abs() < 0.05),
        "precondition: unused should be confident-dead: {:?}",
        analysis.dead_symbols
    );
    let store = Store::open_in_memory().unwrap();
    store
        .save_generation(std::slice::from_ref(&ext), &result, &analysis)
        .unwrap();
    let rows = store.latest_dead_symbols().unwrap();
    let unused = rows
        .iter()
        .find(|row| row.symbol_name == "unused")
        .expect("unused row");
    assert!(
        unused.confidence >= 0.9,
        "API load must not drop 0.9 to 0.899…: {}",
        unused.confidence
    );
    let counted = store.count_dead_at_least(0.9).unwrap();
    assert!(
        counted >= 1,
        "WHERE confidence >= 0.9 must see the HIGH row, got {counted}"
    );
}

#[test]
fn milliconfidence_rounds_the_f32_footgun() {
    assert_eq!(confidence_millis(0.9), 900);
    assert_eq!(Confidence::HIGH.to_millis(), 900);
    assert_eq!(Confidence::HIGH.persist_real(), 0.9);
}

// ---------------------------------------------------------------------------
// 9. Scaling size gate
// ---------------------------------------------------------------------------

#[test]
fn db_size_gate_scales_with_file_count_and_keeps_a_floor() {
    // Derived from the constant, not a repeated literal: the two drifted once
    // already (SC15), and a test that restates the number cannot notice.
    let floor = devmap_extract::model::DB_SIZE_GATE_FLOOR;
    let per_file = devmap_extract::model::DB_SIZE_GATE_PER_FILE;
    let floor_files = floor / per_file;

    assert_eq!(db_size_gate_bytes(0), floor, "an empty repo gets the floor");
    assert_eq!(
        db_size_gate_bytes(floor_files),
        floor,
        "at the crossover the floor still wins"
    );
    assert_eq!(
        db_size_gate_bytes(floor_files + 1),
        (floor_files + 1) * per_file,
        "past the crossover the budget scales per file"
    );
    assert!(db_size_gate_bytes(15_000) > floor);
    // Saturating: a pathological count must not wrap to a tiny budget.
    assert!(db_size_gate_bytes(u64::MAX) >= floor);
}

// ---------------------------------------------------------------------------
// Adversarial / hardening extras
// ---------------------------------------------------------------------------

#[test]
fn go_blank_import_still_emits_an_imports_edge() {
    let importer = extract_file(
        "pkg/a/a.go",
        "package a\nimport _ \"example.com/mod/pkg/b\"\n",
    );
    let target = extract_file("pkg/b/b.go", "package b\nfunc init() {}\n");
    let result = resolve(&[importer, target]);
    assert!(
        result.edges.iter().any(|edge| {
            edge.edge_kind == EdgeKind::Imports
                && edge.source_file == "pkg/a/a.go"
                && edge.target_file == "package:pkg/b/b"
        }),
        "blank import is still a package dependency: {:?}",
        result.edges
    );
}

#[test]
fn go_test_files_are_not_import_targets() {
    let importer = extract_file(
        "pkg/a/a.go",
        "package a\nimport \"example.com/mod/pkg/b\"\n",
    );
    let lib = extract_file("pkg/b/b.go", "package b\nfunc Helper() {}\n");
    let test = extract_file("pkg/b/b_test.go", "package b\nfunc TestHelper() {}\n");
    let result = resolve(&[importer, lib, test]);
    assert!(
        result.edges.iter().any(|edge| {
            edge.edge_kind == EdgeKind::Imports && edge.target_file == "package:pkg/b/b"
        }),
        "the package itself must still resolve: {:?}",
        result.edges
    );
    assert!(
        !result.edges.iter().any(|edge| {
            edge.edge_kind == EdgeKind::Imports && edge.target_file.ends_with("_test.go")
        }),
        "importing a package must not target _test.go files: {:?}",
        result.edges
    );
}

#[test]
fn longest_go_module_prefix_wins_for_nested_modules() {
    let importer = extract_file(
        "mod/nested/pkg/a/a.go",
        "package a\nimport \"example.com/root/nested/pkg/b\"\n",
    );
    let target = extract_file("mod/nested/pkg/b/b.go", "package b\nfunc H() {}\n");
    let modules = [
        GoModule {
            prefix: "example.com/root".into(),
            dir: "mod".into(),
            replaces: vec![],
        },
        GoModule {
            prefix: "example.com/root/nested".into(),
            dir: "mod/nested".into(),
            replaces: vec![],
        },
    ];
    let result = resolve_with_modules(&[importer, target], &modules);
    assert!(
        result.edges.iter().any(|edge| {
            edge.edge_kind == EdgeKind::Imports && edge.target_file == "package:mod/nested/pkg/b/b"
        }),
        "nested module prefix must win: {:?}",
        result.edges
    );
}

#[test]
fn references_do_not_mark_a_symbol_live_from_its_own_declaration_name() {
    let source = extract_file("only.py", "def lonely():\n    return 1\n");
    let result = resolve(std::slice::from_ref(&source));
    let analysis = analyze(std::slice::from_ref(&source), &result);
    assert!(
        confident_dead(&analysis, "lonely").is_some(),
        "the declaration identifier is not a use: {:?}",
        analysis.dead_symbols
    );
}

#[test]
fn go_unexported_unique_global_stays_inside_the_package() {
    let def = extract_file(
        "adkeval/hallucinations.go",
        "package adkeval\nfunc (e *Eval) segment() {}\n",
    );
    let other = extract_file(
        "api/gateway.go",
        "package api\nfunc walk(parts []string) {\n    for _, segment := range parts {\n        _ = segment\n    }\n}\n",
    );
    let result = resolve(&[def, other]);
    assert!(
        !result.edges.iter().any(|edge| {
            edge.edge_kind == EdgeKind::References
                && edge.source_file == "api/gateway.go"
                && edge.target_symbol.contains("segment")
        }),
        "loop variable segment must not bind to another package's method: {:?}",
        result.edges
    );
}

#[test]
fn impact_go_does_not_start_from_every_go_file_path() {
    let files = vec![
        extract_file("alpha.go", "package a\nfunc Alpha() {}\n"),
        extract_file("beta.go", "package b\nfunc Beta() {}\n"),
    ];
    let result = resolve(&files);
    let engine = QueryEngine::new(&files, &result);
    let response = engine.impact(Request {
        query: "go".into(),
        token_budget: 2000,
        min_confidence: 0.0,
        max_depth: 3,
    });
    assert!(
        matches!(
            response.resolution,
            ResolutionAvailability::Unavailable { .. }
        ),
        "`go` must not match *.go file-path node ids: {:?}",
        response.resolution
    );
}

#[test]
fn python_except_alias_does_not_unique_global_to_a_function_named_e() {
    let def = extract_file("assemble.py", "def e():\n    return 1\n");
    let other = extract_file(
        "critics.py",
        "def run():\n    try:\n        missing()\n    except Exception as e:\n        print(e)\n",
    );
    let result = resolve(&[def, other]);
    assert!(
        !result.edges.iter().any(|edge| {
            edge.edge_kind == EdgeKind::References
                && edge.source_file == "critics.py"
                && edge.target_symbol.ends_with("::e")
        }),
        "except-alias e must not unique-global to assemble.py::e: {:?}",
        result.edges
    );
}

#[test]
fn go_range_variable_is_not_a_use_of_a_same_package_function() {
    let def = extract_file("pkg/a.go", "package pkg\nfunc segment() {}\n");
    let other = extract_file(
        "pkg/b.go",
        "package pkg\nfunc walk(parts []string) {\n    for _, segment := range parts {\n        _ = segment\n    }\n}\n",
    );
    let result = resolve(&[def, other]);
    assert!(
        !result.edges.iter().any(|edge| {
            edge.edge_kind == EdgeKind::References
                && edge.source_file == "pkg/b.go"
                && edge.target_symbol.contains("segment")
        }),
        "range variable segment must not bind to func segment in the same package: {:?}",
        result.edges
    );
}

#[test]
fn name_reference_source_is_the_enclosing_callable() {
    let source = extract_file(
        "mod.ts",
        "export type FailureRecord = { ok: boolean };\nexport function report(row: FailureRecord) { return row; }\n",
    );
    let result = resolve(std::slice::from_ref(&source));
    let edge = result
        .edges
        .iter()
        .find(|edge| {
            edge.edge_kind == EdgeKind::References && edge.target_symbol.ends_with("FailureRecord")
        })
        .expect("type annotation must resolve");
    assert!(
        edge.source_symbol.ends_with("::report"),
        "reference source must be the enclosing function, got {}",
        edge.source_symbol
    );
}

#[test]
fn arrow_function_calls_are_attributed_to_the_binding_name() {
    let source = extract_file(
        "Modal.tsx",
        "export function Modal() {\n  const handleClose = () => { persist(); };\n  return null;\n}\nfunction persist() {}\n",
    );
    let result = resolve(std::slice::from_ref(&source));
    let edge = result
        .edges
        .iter()
        .find(|edge| edge.edge_kind == EdgeKind::Calls && edge.target_symbol.ends_with("::persist"))
        .expect("persist() call must resolve");
    // The arrow owns the call, not the function it is declared in. Since SC14
    // the arrow's identity is scoped to that function — `Modal.handleClose`
    // rather than a file-level `handleClose` — so two components each declaring
    // a `handleClose` no longer collapse onto one node.
    assert!(
        edge.source_symbol.ends_with("handleClose"),
        "call inside a named arrow must not collapse onto Modal: {}",
        edge.source_symbol
    );
    assert_ne!(
        edge.source_symbol, "Modal.tsx::Modal",
        "the call must be attributed to the arrow, not to its enclosing function"
    );
    // And that identity must be one a node actually has, or the edge cannot be
    // traversed from the symbol it belongs to.
    assert!(
        source
            .symbols
            .iter()
            .any(|symbol| symbol.qualified_name == edge.source_symbol),
        "edge source {} matches no symbol: {:?}",
        edge.source_symbol,
        source
            .symbols
            .iter()
            .map(|s| &s.qualified_name)
            .collect::<Vec<_>>()
    );
}

#[test]
fn same_file_local_does_not_keep_a_function_of_the_same_name_live() {
    let source = extract_file(
        "assemble.py",
        "def e():\n    return 1\n\ndef run():\n    try:\n        missing()\n    except Exception as e:\n        print(e)\n",
    );
    let result = resolve(std::slice::from_ref(&source));
    assert!(
        !result.edges.iter().any(|edge| {
            edge.edge_kind == EdgeKind::References && edge.target_symbol.ends_with("::e")
        }),
        "except-alias e must not bind to def e in the same file: {:?}",
        result.edges
    );
    let analysis = analyze(std::slice::from_ref(&source), &result);
    assert!(
        confident_dead(&analysis, "e").is_some(),
        "def e is unused; print(e) refers to the local: {:?}",
        analysis.dead_symbols
    );
}

#[test]
fn same_file_function_value_use_without_a_local_still_keeps_it_live() {
    let source = extract_file(
        "assemble.py",
        "def e():\n    return 1\n\ndef run():\n    return e\n",
    );
    let result = resolve(std::slice::from_ref(&source));
    let analysis = analyze(std::slice::from_ref(&source), &result);
    assert!(
        confident_dead(&analysis, "e").is_none(),
        "return e with no local e is a real use of def e: {:?}",
        analysis.dead_symbols
    );
}

#[test]
fn parameter_does_not_keep_a_same_file_function_of_the_same_name_live() {
    let source = extract_file(
        "Modal.tsx",
        "function handleClose() {}\nexport function Modal(handleClose) {\n  return <button onClick={handleClose}>x</button>;\n}\n",
    );
    let result = resolve(std::slice::from_ref(&source));
    let analysis = analyze(std::slice::from_ref(&source), &result);
    assert!(
        confident_dead(&analysis, "handleClose").is_some(),
        "JSX handleClose is the parameter, not the file-level function: {:?}",
        analysis.dead_symbols
    );
}

#[test]
fn go_import_targets_the_package_node_not_every_file() {
    let importer = extract_file(
        "cmd/main.go",
        "package main\nimport \"example.com/mod/pkg/api\"\nfunc Call() { api.Helper() }\n",
    );
    let a = extract_file("pkg/api/a.go", "package api\nfunc Helper() {}\n");
    let b = extract_file("pkg/api/b.go", "package api\nfunc Extra() {}\n");
    let c = extract_file("pkg/api/c.go", "package api\nfunc Other() {}\n");
    let result = resolve(&[importer, a, b, c]);
    let imports: Vec<_> = result
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Imports && edge.source_file == "cmd/main.go")
        .collect();
    assert_eq!(
        imports.len(),
        1,
        "one import spec must not fan out to every package file: {imports:?}"
    );
    assert_eq!(imports[0].target_file, "package:pkg/api/api");
    for file in ["pkg/api/a.go", "pkg/api/b.go", "pkg/api/c.go"] {
        assert!(
            result.edges.iter().any(|edge| {
                edge.edge_kind == EdgeKind::MemberOf
                    && edge.source_file == file
                    && edge.target_file == "package:pkg/api/api"
            }),
            "{file} must still MemberOf the package: {:?}",
            result.edges
        );
    }
    assert!(
        result.edges.iter().any(|edge| {
            edge.edge_kind == EdgeKind::Calls && edge.target_symbol.ends_with("::Helper")
        }),
        "package import must still resolve api.Helper: {:?}",
        result.edges
    );
}
