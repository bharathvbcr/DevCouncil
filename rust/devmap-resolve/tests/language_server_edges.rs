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
        } if target_symbol == "helper"
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
        LspDidNotRun::NonZeroExit { code: Some(1) }.label(),
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
