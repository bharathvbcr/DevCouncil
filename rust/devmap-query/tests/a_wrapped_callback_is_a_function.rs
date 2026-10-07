//! `const f = useCallback(() => …)` binds `f` to a function, and the graph
//! says so.
//!
//! Measured 2026-10-07 on ScholarLM: `App.tsx` declares
//! `const openDocumentGenerationSetup = useCallback(() => { … }, [])` inside
//! `AppContent` and passes it as `onOpenDocumentGeneration={…}`. DevMap had no
//! symbol for it — `search` and `impact` both came back empty — because only a
//! declarator whose value *is* an arrow became a function, and the calls in its
//! body were filed under `AppContent`. `useCallback` wraps 393 arrow consts in
//! that corpus; `memo` and `forwardRef` wrap components the same way.
//!
//! The controls are the wrappers that do not bind a function: `useMemo` returns
//! the arrow's *result*, so its calls run in the enclosing render, and
//! `setTimeout` returns a handle.

use devmap_extract::extract_file;
use devmap_extract::model::SymbolKind;
use devmap_query::{Request, StoreQueryEngine};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};

const APP: &str = "import React, { useCallback, useMemo, memo } from 'react';
import { trackOpen, compute, tick, helper } from './track';

export const AppContent = () => {
  const openDocumentGenerationSetup = useCallback(() => {
    trackOpen('doc');
  }, []);
  const total = useMemo(() => compute(), []);
  const timer = setTimeout(() => tick(), 5);
  const actions = [{ id: 'gen', action: openDocumentGenerationSetup }];
  return <Home onOpenDocumentGeneration={openDocumentGenerationSetup} actions={actions} total={total} timer={timer} />;
};

export const Card = memo(React.forwardRef((props, ref) => {
  return helper(props, ref);
}));

function Home(props: unknown) {
  return null;
}
";

const TRACK: &str = "export function trackOpen(kind: string): void {}
export function compute(): number { return 1; }
export function tick(): void {}
export function helper(a: unknown, b: unknown): null { return null; }
";

fn store() -> Store {
    let extractions = vec![
        extract_file("src/App.tsx", APP),
        extract_file("src/track.ts", TRACK),
    ];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    let store = Store::open_in_memory().unwrap();
    store
        .save_generation_with_opts(
            &extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts::default(),
        )
        .unwrap();
    store
}

fn callers(store: &Store, target: &str) -> Vec<String> {
    let answer = StoreQueryEngine::new(store)
        .impact(Request {
            query: target.to_string(),
            token_budget: 8_000,
            min_confidence: 0.0,
            max_depth: 1,
        })
        .unwrap();
    let mut sources: Vec<String> = answer
        .items
        .iter()
        .map(|edge| edge.source_symbol.clone())
        .collect();
    sources.sort();
    sources.dedup();
    sources
}

#[test]
fn a_use_callback_const_is_one_function_symbol() {
    let extraction = extract_file("src/App.tsx", APP);
    let named: Vec<_> = extraction
        .symbols
        .iter()
        .filter(|symbol| symbol.name == "openDocumentGenerationSetup")
        .map(|symbol| (symbol.qualified_name.as_str(), symbol.kind))
        .collect();
    assert_eq!(
        named,
        vec![(
            "src/App.tsx::AppContent.openDocumentGenerationSetup",
            SymbolKind::Function
        )]
    );
}

#[test]
fn the_calls_in_a_use_callback_body_belong_to_the_callback() {
    let store = store();
    assert_eq!(
        callers(&store, "src/track.ts::trackOpen"),
        vec!["src/App.tsx::AppContent.openDocumentGenerationSetup".to_string()]
    );
}

#[test]
fn the_component_that_passes_the_callback_references_it() {
    let store = store();
    assert_eq!(
        callers(&store, "src/App.tsx::AppContent.openDocumentGenerationSetup"),
        vec!["src/App.tsx::AppContent".to_string()]
    );
}

#[test]
fn a_nested_memo_forward_ref_component_owns_its_body() {
    let extraction = extract_file("src/App.tsx", APP);
    let card: Vec<_> = extraction
        .symbols
        .iter()
        .filter(|symbol| symbol.name == "Card")
        .map(|symbol| (symbol.qualified_name.as_str(), symbol.kind))
        .collect();
    assert_eq!(
        card,
        vec![("src/App.tsx::Card", SymbolKind::Function)],
        "one symbol, not a Variable beside a Function of the same name"
    );
    let store = store();
    assert_eq!(
        callers(&store, "src/track.ts::helper"),
        vec!["src/App.tsx::Card".to_string()]
    );
}

#[test]
fn use_memo_and_set_timeout_bodies_still_belong_to_the_enclosing_render() {
    let extraction = extract_file("src/App.tsx", APP);
    for name in ["total", "timer"] {
        assert!(
            extraction
                .symbols
                .iter()
                .all(|symbol| symbol.name != name || symbol.kind != SymbolKind::Function),
            "`{name}` holds a value, not a function"
        );
    }
    let store = store();
    for callee in ["src/track.ts::compute", "src/track.ts::tick"] {
        assert_eq!(
            callers(&store, callee),
            vec!["src/App.tsx::AppContent".to_string()],
            "{callee}"
        );
    }
}
