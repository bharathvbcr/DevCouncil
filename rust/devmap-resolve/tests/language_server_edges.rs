//! Adversarial cases for the opt-in language-server resolution pass.
//!
//! None of these write a `UniqueGlobal` edge. Unresolved sites stay in the
//! ledger unless a `LanguageServer` edge actually names a target.

use std::collections::BTreeMap;
use std::sync::Arc;

use devmap_extract::model::{EdgeKind, ExtractedSymbol, Span, SymbolKind};
use devmap_resolve::lsp::{
    apply_language_server_resolutions, decide_language_server_resolution, lsp_edge_details,
    reconcile_server_answers, LspDidNotRun, LspLocation, LspSiteAnswer, LSP_PAYLOAD_CAP,
};
use devmap_resolve::model::{
    Resolution, ResolutionKind, ResolutionResult, UnresolvedClass, UnresolvedKind,
    UnresolvedReference,
};

fn symbol(file: &str, name: &str, start: usize, end: usize) -> ExtractedSymbol {
    ExtractedSymbol {
        name: name.to_string(),
        qualified_name: format!("{file}::{name}"),
        kind: SymbolKind::Function,
        span: Span {
            start_byte: start,
            end_byte: end,
        },
        is_exported: true,
        docstring: None,
        signature: None,
        parent_symbol: None,
        body_signature: None,
        declaration_hash: None,
        return_type: None,
    }
}

fn unresolved_site(file: &str, caller: &str, callee: &str) -> UnresolvedReference {
    UnresolvedReference {
        source_file: file.to_string(),
        source_symbol: caller.to_string(),
        callee_name: callee.to_string(),
        kind: UnresolvedKind::Call,
        resolution: Resolution::Unresolved {
            reason: "no declaration".into(),
        },
        class: UnresolvedClass::Unresolved,
        receiver: None,
        site: None,
    }
}

#[test]
fn a_target_outside_the_repo_never_becomes_a_unique_global() {
    // The client drops out-of-repo URIs before decide sees them. An empty
    // location list must abstain — never invent UniqueGlobal from the callee.
    let answer = LspSiteAnswer {
        server: "rust-analyzer".into(),
        server_version: "1.0.0".into(),
        locations: vec![],
    };
    let symbols: BTreeMap<&str, Vec<&ExtractedSymbol>> = BTreeMap::new();
    let sources: BTreeMap<String, String> = BTreeMap::new();
    let decided = decide_language_server_resolution(&answer, &symbols, &sources);
    assert!(decided.is_none());
    assert!(!matches!(decided, Some(Resolution::UniqueGlobal { .. })));
}

#[test]
fn two_servers_that_disagree_abstain_rather_than_picking_a_winner() {
    let a = LspSiteAnswer {
        server: "rust-analyzer".into(),
        server_version: "1".into(),
        locations: vec![LspLocation {
            file: "a.rs".into(),
            line: 0,
            character: 0,
        }],
    };
    let b = LspSiteAnswer {
        server: "pyright".into(),
        server_version: "1".into(),
        locations: vec![LspLocation {
            file: "b.py".into(),
            line: 0,
            character: 0,
        }],
    };
    assert!(reconcile_server_answers(&[a, b]).is_none());
}

#[test]
fn one_in_repo_hit_that_lands_on_a_symbol_is_language_server_not_unique_global() {
    let helper = symbol("lib.rs", "helper", 0, 20);
    let symbols: BTreeMap<&str, Vec<&ExtractedSymbol>> =
        BTreeMap::from([("lib.rs", vec![&helper])]);
    let sources = BTreeMap::from([("lib.rs".into(), "fn helper() {}\n".into())]);
    let answer = LspSiteAnswer {
        server: "rust-analyzer".into(),
        server_version: "0.3.0".into(),
        locations: vec![LspLocation {
            file: "lib.rs".into(),
            line: 0,
            character: 3,
        }],
    };
    let decided =
        decide_language_server_resolution(&answer, &symbols, &sources).expect("one hit must bind");
    assert!(matches!(
        decided,
        Resolution::LanguageServer {
            ref target_symbol,
            ref target_file,
            ref server,
            ..
        } if target_symbol == "lib.rs::helper"
            && target_file == "lib.rs"
            && server == "rust-analyzer"
    ));
    assert_eq!(decided.kind(), ResolutionKind::LanguageServer);
    assert_ne!(decided.kind(), ResolutionKind::UniqueGlobal);
}

#[test]
fn several_hits_are_language_server_dispatch_not_unique_global() {
    let a = symbol("a.rs", "run", 0, 10);
    let b = symbol("b.rs", "run", 0, 10);
    let symbols: BTreeMap<&str, Vec<&ExtractedSymbol>> =
        BTreeMap::from([("a.rs", vec![&a]), ("b.rs", vec![&b])]);
    let sources = BTreeMap::from([
        ("a.rs".into(), "fn run() {}\n".into()),
        ("b.rs".into(), "fn run() {}\n".into()),
    ]);
    let answer = LspSiteAnswer {
        server: "rust-analyzer".into(),
        server_version: "0.3.0".into(),
        locations: vec![
            LspLocation {
                file: "a.rs".into(),
                line: 0,
                character: 3,
            },
            LspLocation {
                file: "b.rs".into(),
                line: 0,
                character: 3,
            },
        ],
    };
    let decided = decide_language_server_resolution(&answer, &symbols, &sources)
        .expect("several hits must dispatch");
    assert!(matches!(decided, Resolution::LanguageServerDispatch { .. }));
    assert_eq!(decided.kind(), ResolutionKind::LanguageServerDispatch);
    assert_eq!(
        decided.kind().confidence(),
        ResolutionKind::AmbiguousGlobal.confidence()
    );
}

#[test]
fn applying_a_language_server_edge_shrinks_the_ledger_and_records_the_server() {
    let mut resolution = ResolutionResult {
        edges: Vec::new(),
        receiver_types: Default::default(),
        reexport_chains: Default::default(),
        unresolved: vec![unresolved_site("main.rs", "main", "helper")],
    };
    let decided = Resolution::LanguageServer {
        target_symbol: "helper".into(),
        target_file: "lib.rs".into(),
        server: "rust-analyzer".into(),
        server_version: "1.2.3".into(),
    };
    let added = apply_language_server_resolutions(&mut resolution, &[(0, decided)]);
    assert_eq!(added, 1);
    assert!(
        resolution.unresolved.is_empty(),
        "ledger must shrink only for named targets"
    );
    assert_eq!(resolution.edges.len(), 1);
    assert_eq!(
        resolution.edges[0].details.as_deref(),
        Some(lsp_edge_details("rust-analyzer", "1.2.3").as_str())
    );
    assert_eq!(
        resolution.edges[0].evidence.map(|e| e.kind),
        Some(ResolutionKind::LanguageServer)
    );
    assert!(!matches!(
        resolution.edges[0].resolution.as_deref(),
        Some(Resolution::UniqueGlobal { .. })
    ));
}

#[test]
fn applying_nothing_leaves_the_ledger_intact() {
    let mut resolution = ResolutionResult {
        edges: Vec::new(),
        receiver_types: Default::default(),
        reexport_chains: Default::default(),
        unresolved: vec![unresolved_site("main.rs", "main", "helper")],
    };
    let before = resolution.unresolved.len();
    let added = apply_language_server_resolutions(&mut resolution, &[]);
    assert_eq!(added, 0);
    assert_eq!(resolution.unresolved.len(), before);
    assert!(resolution.edges.is_empty());
}

#[test]
fn oversized_payload_reason_is_distinct_from_a_successful_zero_edge_run() {
    let reason = LspDidNotRun::OversizedPayload {
        bytes: LSP_PAYLOAD_CAP + 1,
    };
    assert_eq!(reason.label(), "oversized_payload");
    assert_ne!(reason.label(), "missing_binary");
}

#[test]
fn missing_binary_and_non_zero_exit_are_did_not_run_labels() {
    assert_eq!(LspDidNotRun::MissingBinary.label(), "missing_binary");
    assert_eq!(
        LspDidNotRun::NonZeroExit {
            code: Some(1),
            stderr: None,
            stderr_complete: true,
            stderr_not_shown: 0
        }
        .label(),
        "non_zero_exit"
    );
}

#[test]
fn language_server_edges_are_calls_with_high_confidence() {
    let decided = Arc::new(Resolution::LanguageServer {
        target_symbol: "helper".into(),
        target_file: "lib.rs".into(),
        server: "gopls".into(),
        server_version: "0.1".into(),
    });
    let mut resolution = ResolutionResult {
        edges: Vec::new(),
        receiver_types: Default::default(),
        reexport_chains: Default::default(),
        unresolved: vec![unresolved_site("main.go", "main", "helper")],
    };
    apply_language_server_resolutions(&mut resolution, &[(0, (*decided).clone())]);
    assert_eq!(resolution.edges[0].edge_kind, EdgeKind::Calls);
    assert_eq!(
        resolution.edges[0].confidence,
        ResolutionKind::LanguageServer.confidence()
    );
}

fn kinded(file: &str, name: &str, kind: SymbolKind, start: usize, end: usize) -> ExtractedSymbol {
    ExtractedSymbol {
        kind,
        ..symbol(file, name, start, end)
    }
}

fn answer_at(file: &str, line: u32, character: u32) -> LspSiteAnswer {
    LspSiteAnswer {
        server: "gopls".into(),
        server_version: "0.1".into(),
        locations: vec![LspLocation {
            file: file.into(),
            line,
            character,
        }],
    }
}

/// 88% of the pairs `--lsp` added on the Go snapshot targeted a `File` or a
/// `Struct`: a definition that is a type, a local or an import has no function
/// around it, so the innermost symbol of *any* kind was the file or the type,
/// and the call was bound to that.
#[test]
fn a_definition_that_is_a_type_or_a_file_binds_nothing() {
    let source = "package p\ntype args struct{}\nvar out args\n";
    let file = kinded("p.go", "p.go", SymbolKind::File, 0, source.len());
    let ty = kinded("p.go", "args", SymbolKind::Struct, 10, 29);
    let symbols: BTreeMap<&str, Vec<&ExtractedSymbol>> =
        BTreeMap::from([("p.go", vec![&file, &ty])]);
    let sources = BTreeMap::from([("p.go".to_string(), source.to_string())]);

    // `args` in `type args struct{}` (line 1, col 5): the struct.
    assert!(
        decide_language_server_resolution(&answer_at("p.go", 1, 5), &symbols, &sources).is_none()
    );
    // `out` in `var out args` (line 2, col 4): only the file holds it.
    assert!(
        decide_language_server_resolution(&answer_at("p.go", 2, 4), &symbols, &sources).is_none()
    );
}

/// A local declared inside a function is "inside" it, and is not it: binding
/// the call to the enclosing function would be a fabricated self-call.
#[test]
fn a_local_declared_inside_a_function_does_not_bind_to_that_function() {
    let source = "package p\nfunc outer() {\n\thandler := func() {}\n\thandler()\n}\n";
    let file = kinded("p.go", "p.go", SymbolKind::File, 0, source.len());
    let outer = kinded("p.go", "outer", SymbolKind::Function, 10, source.len() - 1);
    let symbols: BTreeMap<&str, Vec<&ExtractedSymbol>> =
        BTreeMap::from([("p.go", vec![&file, &outer])]);
    let sources = BTreeMap::from([("p.go".to_string(), source.to_string())]);
    // `handler` at line 2, column 1.
    assert!(
        decide_language_server_resolution(&answer_at("p.go", 2, 1), &symbols, &sources).is_none()
    );
}

/// The target is stored by its qualified name, as every other rung stores it,
/// so the edge joins the node the store holds; a method keeps its type.
#[test]
fn a_method_target_is_stored_by_its_qualified_name() {
    let source = "package p\nfunc (s *S) Close() {}\n";
    let mut method = kinded("p.go", "Close", SymbolKind::Method, 10, source.len() - 1);
    method.qualified_name = "p.go::S.Close".into();
    let symbols: BTreeMap<&str, Vec<&ExtractedSymbol>> = BTreeMap::from([("p.go", vec![&method])]);
    let sources = BTreeMap::from([("p.go".to_string(), source.to_string())]);
    let decided = decide_language_server_resolution(&answer_at("p.go", 1, 12), &symbols, &sources)
        .expect("a method's own name binds");
    assert!(matches!(
        decided,
        Resolution::LanguageServer { ref target_symbol, .. } if target_symbol == "p.go::S.Close"
    ));
}
