//! W0.4 — the defect ledger vetoes a confident dead verdict.
//!
//! The kernel records every call site the resolution ladder could not bind, in
//! six classified tiers, and the dead-code pass never read a row of it. If an
//! unresolved site names `foo`, then "nothing calls `foo`" is a statement about
//! the resolver, not about the code — and it was being published at the
//! `extracted` tier, whose contract is "safe to act on".
//!
//! The veto is deliberately narrow. Only the two classes where the resolver
//! *admits it does not know* qualify:
//!
//! * `UninferredReceiver` — the receiver exists and could not be typed.
//! * `Unresolved` — a bare name nothing explains; the ledger's own docs call it
//!   "the only tier that indicates a defect".
//!
//! The other four carry affirmative evidence that the site meant something
//! else, and admitting them would swallow real findings on a coincidence of
//! spelling. Each is guarded below by name, because a veto that fires on
//! everything is indistinguishable from having deleted dead-code detection —
//! and that failure would be silent.

use devmap_analyze::*;
use devmap_extract::extract_file;
use devmap_extract::model::*;
use devmap_resolve::model::*;
use devmap_resolve::Resolver;

fn analyze_with(extractions: &[Extraction]) -> Vec<DeadSymbolReport> {
    let mut resolver = Resolver::new();
    resolver.index_extractions(extractions);
    let resolution = resolver.resolve_all(extractions);
    analyze_liveness(extractions, &resolution)
}

/// Run liveness against a hand-built ledger, so each class can be exercised in
/// isolation.
///
/// Building the row directly rather than coaxing the resolver into producing it
/// is the only way to test the *filter*: `Builtin` and `HostGlobal` rows are
/// produced for names the corpus does not declare, so a fixture that made the
/// resolver emit one naturally could not also declare a symbol of that name.
fn analyze_with_ledger(
    extractions: &[Extraction],
    rows: Vec<UnresolvedReference>,
) -> Vec<DeadSymbolReport> {
    let mut resolver = Resolver::new();
    resolver.index_extractions(extractions);
    let mut resolution = resolver.resolve_all(extractions);
    resolution.unresolved = rows;
    analyze_liveness(extractions, &resolution)
}

fn row(callee: &str, kind: UnresolvedKind, class: UnresolvedClass) -> UnresolvedReference {
    UnresolvedReference {
        source_file: "caller.py".to_string(),
        source_symbol: "caller.py::main".to_string(),
        callee_name: callee.to_string(),
        kind,
        resolution: Resolution::Unresolved {
            reason: "test fixture".to_string(),
        },
        class,
        receiver: None,
    }
}

/// `lib.py` declares `render`, which nothing in the corpus calls by a bindable
/// name.
fn corpus() -> Vec<Extraction> {
    vec![extract_file(
        "lib.py",
        "def render(name):\n    return name\n\n\ndef other():\n    return 2\n",
    )]
}

fn finding<'a>(reports: &'a [DeadSymbolReport], symbol: &str) -> &'a DeadSymbolReport {
    reports
        .iter()
        .find(|r| r.symbol_name == symbol)
        .unwrap_or_else(|| panic!("no finding for {symbol}: {reports:?}"))
}

/// The baseline: with an empty ledger, `render` is a confident finding.
///
/// This is the assertion every other test here is measured against. Without it
/// a veto that fired unconditionally would look identical to one that worked.
#[test]
fn with_no_unresolved_sites_the_finding_is_confident() {
    let reports = analyze_with(&corpus());
    let render = finding(&reports, "render");
    assert!(!render.is_exempt);
    assert_eq!(
        render.confidence, 0.9,
        "an uncalled symbol in a fully-resolved corpus is the confident case"
    );
    assert_eq!(render.exemption_reason, None);
}

/// The veto: an uninferrable receiver naming `render` demotes it.
#[test]
fn an_uninferred_receiver_naming_the_symbol_vetoes_the_confident_tier() {
    let reports = analyze_with_ledger(
        &corpus(),
        vec![row(
            "render",
            UnresolvedKind::Call,
            UnresolvedClass::UninferredReceiver,
        )],
    );
    let render = finding(&reports, "render");
    assert_eq!(
        render.confidence, 0.4,
        "a call the resolver could not bind is evidence something reaches this"
    );
    assert_eq!(
        render.exemption_reason.as_deref(),
        Some(UNRESOLVED_NAMESAKE_REASON)
    );

    // And the veto is scoped to the name, not to the corpus: `other` shares the
    // file and keeps its confident finding.
    let other = finding(&reports, "other");
    assert_eq!(
        other.confidence, 0.9,
        "one unresolved name must not demote every symbol beside it"
    );
}

/// A bare unresolved name does the same.
#[test]
fn a_bare_unresolved_call_naming_the_symbol_vetoes_it() {
    let reports = analyze_with_ledger(
        &corpus(),
        vec![row(
            "render",
            UnresolvedKind::Call,
            UnresolvedClass::Unresolved,
        )],
    );
    assert_eq!(finding(&reports, "render").confidence, 0.4);
}

/// An unbound route handler is the strongest case of all.
///
/// `HandlesRoute` is what tells liveness a handler is reached from outside the
/// call graph. A route whose handler name did not bind leaves a live handler
/// looking dead — the ledger's own documentation says so — so `Route` is
/// admitted alongside `Call` and `Reference`.
#[test]
fn an_unbound_route_handler_naming_the_symbol_vetoes_it() {
    let reports = analyze_with_ledger(
        &corpus(),
        vec![row(
            "render",
            UnresolvedKind::Route,
            UnresolvedClass::Unresolved,
        )],
    );
    assert_eq!(finding(&reports, "render").confidence, 0.4);
}

/// The inverse guard, class by class.
///
/// Each of these four classes carries evidence that the site meant something
/// other than the corpus symbol. If any of them vetoed, the ledger's largest
/// tiers would suppress findings wholesale: on this repository `LocalBinding`
/// and `External` alone account for the bulk of unresolved rows, so admitting
/// them would quietly retire dead-code detection while every test that only
/// checks the ON direction kept passing.
#[test]
fn classes_with_affirmative_evidence_do_not_veto() {
    for (label, class) in [
        // Declared by the language itself; no indexed file can declare these.
        ("builtin", UnresolvedClass::Builtin),
        (
            "host_global",
            UnresolvedClass::HostGlobal {
                environment: "node".to_string(),
            },
        ),
        // The enclosing symbol declares the name itself — a parameter or a
        // local closure. The ladder failing here is the correct outcome.
        ("local_binding", UnresolvedClass::LocalBinding),
        // Bound by an import naming no indexed file; the import is the
        // evidence. A repo-relative specifier is filed under `Unresolved`
        // instead, precisely so this class stays import-proven.
        (
            "external",
            UnresolvedClass::External {
                module: "react".to_string(),
            },
        ),
    ] {
        let reports =
            analyze_with_ledger(&corpus(), vec![row("render", UnresolvedKind::Call, class)]);
        let render = finding(&reports, "render");
        assert_eq!(
            render.confidence, 0.9,
            "{label}: this class explains itself, so it is not evidence that \
             anything calls `render`; vetoing on it would swallow real findings"
        );
        assert_eq!(render.exemption_reason, None, "{label}");
    }
}

/// An unresolved *import specifier* must not veto.
///
/// `UnresolvedKind::Import` stores a module specifier in `callee_name`, not a
/// symbol name. Matching specifiers against symbols is noise, and a repository
/// with a `./render` module would otherwise suppress every `render` in it.
#[test]
fn an_unresolved_import_specifier_does_not_veto() {
    let reports = analyze_with_ledger(
        &corpus(),
        vec![row(
            "render",
            UnresolvedKind::Import,
            UnresolvedClass::Unresolved,
        )],
    );
    assert_eq!(finding(&reports, "render").confidence, 0.9);
}

/// A different name does not veto.
///
/// The join is by name; a veto that ignored the name would be a corpus-wide
/// switch, which is the failure mode this whole file guards.
#[test]
fn an_unresolved_site_naming_something_else_does_not_veto() {
    let reports = analyze_with_ledger(
        &corpus(),
        vec![row(
            "unrelated",
            UnresolvedKind::Call,
            UnresolvedClass::Unresolved,
        )],
    );
    assert_eq!(finding(&reports, "render").confidence, 0.9);
}

/// An empty callee name matches nothing.
///
/// `HashSet<&str>` would happily hold `""`, and a symbol can never be named
/// `""` — but a row with an empty name is a defect in the ledger, and admitting
/// it costs nothing to exclude and would be invisible if it ever started
/// matching through some future normalisation.
#[test]
fn an_empty_callee_name_is_not_a_namesake() {
    let reports = analyze_with_ledger(
        &corpus(),
        vec![row("", UnresolvedKind::Call, UnresolvedClass::Unresolved)],
    );
    assert_eq!(finding(&reports, "render").confidence, 0.9);
}

/// The veto reason states the imprecision, in the reason itself.
///
/// `UnresolvedReference` carries no `target_file`, so the join is name-only and
/// corpus-wide. A reader acting on a demoted finding needs that in the string
/// they read, not in a comment beside the code that produced it.
#[test]
fn the_reason_admits_the_join_is_name_only_and_corpus_wide() {
    for fragment in ["name", "corpus", "no target file"] {
        assert!(
            UNRESOLVED_NAMESAKE_REASON.contains(fragment),
            "the reason must state its own imprecision ({fragment}): \
             {UNRESOLVED_NAMESAKE_REASON}"
        );
    }
}

/// An exported symbol is untouched by the veto.
///
/// The veto only reaches the branch that would otherwise be confident. If it
/// leaked into the exempt branch it would rewrite reasons for symbols that were
/// never findings.
#[test]
fn the_veto_does_not_touch_exempt_symbols() {
    let extractions = vec![extract_file(
        "lib.py",
        "__all__ = ['render']\n\n\ndef render(name):\n    return name\n",
    )];
    let reports = analyze_with_ledger(
        &extractions,
        vec![row(
            "render",
            UnresolvedKind::Call,
            UnresolvedClass::Unresolved,
        )],
    );
    if let Some(render) = reports.iter().find(|r| r.symbol_name == "render") {
        assert_ne!(
            render.exemption_reason.as_deref(),
            Some(UNRESOLVED_NAMESAKE_REASON),
            "an exported symbol keeps its own exemption reason: {render:?}"
        );
    }
}

/// End to end, through the real resolver, on a shape taken from this repository.
///
/// Every test above injects the ledger so one class can be exercised in
/// isolation. This one does not — and it is the test that proves the veto is
/// reachable at all rather than dead configuration.
///
/// The shape is `devmap-store/src/edge_index.rs:430`, reduced: a nested `fn`
/// declared inside a method and called from it several lines later. The
/// extractor emits the nested function as a symbol, the resolver cannot bind
/// the bare calls to it, and so the pre-veto kernel reported
/// `GenerationEdges.map_bytes` as dead — a function called four times, in the
/// same file, in this very workspace. Measured across the 474-file `rust-port`
/// corpus, 303 unresolved sites name a declared symbol and this was the one
/// that reached a finding.
#[test]
fn the_veto_fires_on_what_the_real_resolver_produces() {
    let extractions = vec![extract_file(
        "edge_index.rs",
        "pub struct GenerationEdges {\n    a: Vec<u32>,\n    b: Vec<u32>,\n}\n\n\
         impl GenerationEdges {\n    pub fn heap_bytes(&self) -> usize {\n        \
         fn map_bytes(m: &Vec<u32>) -> usize {\n            m.len() * 4\n        }\n        \
         map_bytes(&self.a) + map_bytes(&self.b)\n    }\n}\n",
    )];

    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);

    let named: Vec<&UnresolvedReference> = resolution
        .unresolved
        .iter()
        .filter(|r| r.callee_name == "map_bytes")
        .collect();
    assert!(
        !named.is_empty(),
        "the resolver must actually record these calls as unresolved, or the \
         veto has nothing to read: {:?}",
        resolution.unresolved
    );
    assert!(
        named.iter().any(|r| matches!(
            r.class,
            UnresolvedClass::UninferredReceiver | UnresolvedClass::Unresolved
        )),
        "the row must land in a class the veto admits: {named:?}"
    );

    let report = analyze_liveness(&extractions, &resolution)
        .into_iter()
        .find(|r| r.symbol_name.contains("map_bytes"))
        .expect("the nested function must still be reported, not hidden");
    assert_eq!(
        report.exemption_reason.as_deref(),
        Some(UNRESOLVED_NAMESAKE_REASON),
        "a function whose only callers the resolver could not bind must not be \
         proposed for deletion at the confident tier: {report:?}"
    );
    assert!(report.confidence <= 0.4, "{report:?}");
}
