//! `impact` on a component rendered as a JSX tag finds the component that
//! renders it.
//!
//! Measured 2026-10-07 on ScholarLM: `App.tsx` imports `HomePage` with a plain
//! named import and renders `<HomePage …/>` inside `AppContent`, and the
//! ledger filed that tag `local_binding` — "the callee is a local binding whose
//! value is not known". The extractor read the tag's `name` field as a
//! declaration, so `HomePage` became a local of every component that rendered
//! it and the resolver refused the tag's own call. 3,686 TSX sites were filed
//! that way, among them every imported component rendered inside another one.
//!
//! The same misreading hit C#: `other.Format` made `Format` a local of the
//! method, and a bare `Format(…)` call later in it went unresolved.
//!
//! The controls are the two shapes that must not change: a component that
//! really is a local (`const Local = pick(); <Local/>`) stays unresolved rather
//! than binding to a namesake elsewhere, and a tag still records exactly one
//! `JsxTag` reference and no `Name` reference of its own.

use devmap_extract::extract_file;
use devmap_extract::model::ReferenceKind;
use devmap_query::{Request, StoreQueryEngine};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};

const APP: &str = "import React from 'react';
import { HomePage } from './HomePage';
import { Layout } from './Layout';

export const AppContent = () => {
  return (
    <Layout>
      <HomePage title=\"x\" />
    </Layout>
  );
};

export function Shadowing() {
  const HomePage = pick();
  return <HomePage />;
}

function pick() {
  return null;
}
";

const HOME: &str = "export const HomePage = ({ title }: { title: string }) => {
  return <div>{title}</div>;
};
";

const LAYOUT: &str = "export function Layout({ children }: { children: unknown }) {
  return <main>{children}</main>;
}
";

const RUNNER: &str = "namespace Demo
{
    public class Runner
    {
        public string Label = \"x\";

        public static string Format(string s) { return s.Trim(); }

        public string Run(Runner other)
        {
            var label = other.Format;
            return Format(other.Label);
        }
    }
}
";

fn store_of(files: &[(&str, &str)]) -> Store {
    let extractions: Vec<_> = files
        .iter()
        .map(|(path, body)| extract_file(path, body))
        .collect();
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

fn tsx_store() -> Store {
    store_of(&[
        ("src/App.tsx", APP),
        ("src/HomePage.tsx", HOME),
        ("src/Layout.tsx", LAYOUT),
    ])
}

#[test]
fn an_imported_component_rendered_inside_a_component_has_that_caller() {
    let store = tsx_store();
    assert_eq!(
        callers(&store, "src/HomePage.tsx::HomePage"),
        vec!["src/App.tsx::AppContent".to_string()],
        "the `<HomePage/>` in `Shadowing` renders its own local and must not be counted"
    );
}

#[test]
fn a_component_with_a_closing_tag_has_its_caller() {
    let store = tsx_store();
    assert_eq!(
        callers(&store, "src/Layout.tsx::Layout"),
        vec!["src/App.tsx::AppContent".to_string()]
    );
}

#[test]
fn a_tag_that_renders_a_real_local_stays_unresolved() {
    let extraction = extract_file("src/App.tsx", APP);
    let shadowing_tag = extraction
        .calls
        .iter()
        .find(|call| {
            call.callee_name == "HomePage"
                && call.caller_symbol.as_deref() == Some("src/App.tsx::Shadowing")
        })
        .expect("the tag inside `Shadowing` is a call");
    assert!(
        extraction
            .local_binding_at(shadowing_tag.span.start_byte, "HomePage")
            .is_some(),
        "`const HomePage = pick()` is a local, and the tag reads it"
    );
    let rendering_tag = extraction
        .calls
        .iter()
        .find(|call| {
            call.callee_name == "HomePage"
                && call.caller_symbol.as_deref() == Some("src/App.tsx::AppContent")
        })
        .expect("the tag inside `AppContent` is a call");
    assert!(
        extraction
            .local_binding_at(rendering_tag.span.start_byte, "HomePage")
            .is_none(),
        "the tag inside `AppContent` reads the import, not a local"
    );
}

#[test]
fn a_tag_is_one_jsx_reference_and_no_name_reference() {
    let extraction = extract_file("src/App.tsx", APP);
    let layout: Vec<_> = extraction
        .references
        .iter()
        .filter(|reference| reference.name == "Layout")
        .map(|reference| reference.kind)
        .collect();
    assert_eq!(
        layout,
        vec![ReferenceKind::JsxTag],
        "`<Layout>…</Layout>` is one opening tag; the closing tag names nothing new"
    );
}

#[test]
fn a_csharp_member_read_does_not_shadow_a_bare_call_of_the_same_name() {
    let store = store_of(&[("Runner.cs", RUNNER)]);
    assert_eq!(
        callers(&store, "Runner.cs::Runner.Format"),
        vec!["Runner.cs::Runner.Run".to_string()]
    );
}
