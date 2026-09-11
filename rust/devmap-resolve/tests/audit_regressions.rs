//! Regressions for the resolve-crate audit findings R-1 … R-12.
//!
//! Every test here was written against the **unmodified** resolver and watched
//! to fail before the fix that makes it pass. Each one carries a positive
//! control, so a fix that simply refuses everything cannot turn it green.

use devmap_extract::extract_file;
use devmap_extract::model::*;
use devmap_extract::GoModule;
use devmap_resolve::*;

fn extractions(files: &[(&str, &str)]) -> Vec<Extraction> {
    files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect()
}

fn resolve(files: &[(&str, &str)]) -> ResolutionResult {
    let exts = extractions(files);
    let mut resolver = Resolver::new();
    resolver.index_extractions(&exts);
    resolver.resolve_all(&exts)
}

fn calls(result: &ResolutionResult) -> Vec<String> {
    let mut rows: Vec<String> = result
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls)
        .map(|edge| format!("{}->{}", edge.source_symbol, edge.target_symbol))
        .collect();
    rows.sort();
    rows
}

/// The confidence each resolution tier is entitled to.
///
/// Deliberately an *independent* table rather than a call into the crate: the
/// crate's own mapping is the thing under test, so using it as the oracle would
/// make this tautological. The crate is asserted to agree with it below.
fn entitled_confidence(resolution: &Resolution) -> Confidence {
    match resolution {
        // Evidence names exactly one target: a declaration in this very file,
        // an import statement in it, or a receiver whose type is known.
        Resolution::SameFile { .. }
        | Resolution::ImportScoped { .. }
        | Resolution::ReceiverType { .. } => Confidence::DETERMINISTIC,
        // X45. Go's spec puts a package-level identifier in scope, unqualified,
        // throughout the package — a scope rule the file's own package clause
        // states, not a count of matches across the family.
        Resolution::SamePackage { .. } => Confidence::DETERMINISTIC,
        // Not a resolved reference: a relation the graph asserts about its own
        // shape, from a declaration the file carries outright (a Go package
        // clause). Deterministic for the same reason the three above are.
        Resolution::Structural { .. } => Confidence::DETERMINISTIC,
        // One match across the whole family, with nothing tying it to this
        // file. Strong, not certain.
        Resolution::UniqueGlobal { .. } => Confidence::HIGH,
        Resolution::AmbiguousGlobal { .. } => Confidence::SPECULATIVE,
        Resolution::Unresolved { .. } => Confidence::SPECULATIVE,
    }
}

/// R-1: a bare-name `References` edge must not be stamped DETERMINISTIC.
///
/// `fn take(s: Shape)` with no import, against a `Shape` declared in another
/// file, is the *same evidence* the call ladder rates HIGH. Stamping it 1.0
/// makes a fabricated reference outrank an honest call: a `min_confidence=1.0`
/// query keeps the reference and drops the call.
#[test]
fn a_unique_global_reference_is_not_deterministic() {
    let result = resolve(&[
        ("u.rs", "pub fn take(s: Shape) -> Shape { s }\n"),
        ("t.rs", "pub struct Shape;\n"),
    ]);
    let edge = result
        .edges
        .iter()
        .find(|edge| edge.edge_kind == EdgeKind::References && edge.target_file == "t.rs")
        .expect("the reference must still resolve — this is about its confidence, not its target");
    assert!(
        matches!(
            edge.resolution.as_deref(),
            Some(Resolution::UniqueGlobal { .. })
        ),
        "fixture must exercise the UniqueGlobal rung, got {:?}",
        edge.resolution
    );
    assert_eq!(
        edge.confidence,
        Confidence::HIGH,
        "a bare-name reference resolved only by being globally unique carries \
         the same evidence as a UniqueGlobal call, which is HIGH"
    );

    // Positive control: same-file evidence still earns DETERMINISTIC.
    let same_file = resolve(&[(
        "one.ts",
        "export class Rec {}\nexport function use(r: Rec) { return r; }\n",
    )]);
    let local = same_file
        .edges
        .iter()
        .find(|edge| edge.edge_kind == EdgeKind::References)
        .expect("a same-file annotation resolves");
    assert!(
        matches!(
            local.resolution.as_deref(),
            Some(Resolution::SameFile { .. })
        ),
        "the control must exercise the SameFile rung, got {:?}",
        local.resolution
    );
    assert_eq!(
        local.confidence,
        Confidence::DETERMINISTIC,
        "a declaration in this very file is a fact"
    );
}

/// R-1 (second family): the rung, not the language, decides the confidence.
#[test]
fn a_unique_global_reference_is_not_deterministic_in_go_either() {
    let result = resolve(&[
        ("u.go", "package a\nfunc Take(s Shape) Shape { return s }\n"),
        ("t/t.go", "package t\ntype Shape struct{}\n"),
    ]);
    let edge = result
        .edges
        .iter()
        .find(|edge| edge.edge_kind == EdgeKind::References && edge.target_file == "t/t.go")
        .expect("the Go type reference must still resolve");
    assert_eq!(
        edge.confidence,
        Confidence::HIGH,
        "a type found only by global uniqueness is not a certainty"
    );
}

/// R-8: every edge's confidence is the one its resolution entitles it to.
///
/// `ResolvedEdge` carries `confidence` and `resolution` as independent fields,
/// so nothing in the type prevents `AmbiguousGlobal` at 1.0. This is the
/// invariant that must hold across every rung the resolver can reach.
#[test]
fn every_edge_confidence_matches_the_evidence_it_names() {
    let result = resolve(&[
        (
            "app.py",
            "from models import Record\nimport helpers\n\n\
             class Holder:\n    def take(self, r):\n        return r\n\n\
             def use(r: Record):\n    h = Holder()\n    h.take(r)\n    helpers.do()\n    \
             return shared()\n",
        ),
        ("models.py", "class Record:\n    pass\n"),
        ("helpers.py", "def do():\n    return 1\n"),
        ("x.py", "def shared():\n    pass\n"),
        ("y.py", "def shared():\n    pass\n"),
        ("free.ts", "export class Free {}\n"),
        ("u.ts", "export function annotate(f: Free) { return f; }\n"),
    ]);
    assert!(
        result.edges.len() > 10,
        "the fixture must actually exercise the ladder, got {} edges",
        result.edges.len()
    );
    let mut seen_tiers: Vec<String> = Vec::new();
    for edge in &result.edges {
        let resolution = edge
            .resolution
            .as_deref()
            .expect("every emitted edge names its evidence");
        let tier = match resolution {
            Resolution::SameFile { .. } => "SameFile",
            Resolution::SamePackage { .. } => "SamePackage",
            Resolution::ImportScoped { .. } => "ImportScoped",
            Resolution::ReceiverType { .. } => "ReceiverType",
            Resolution::UniqueGlobal { .. } => "UniqueGlobal",
            Resolution::AmbiguousGlobal { .. } => "AmbiguousGlobal",
            Resolution::Unresolved { .. } => "Unresolved",
            Resolution::Structural { .. } => "Structural",
        };
        if !seen_tiers.iter().any(|seen| seen == tier) {
            seen_tiers.push(tier.to_string());
        }
        assert_eq!(
            edge.confidence,
            entitled_confidence(resolution),
            "{} -> {} ({:?}) claims {:?} on evidence {:?}",
            edge.source_symbol,
            edge.target_symbol,
            edge.edge_kind,
            edge.confidence,
            resolution
        );
    }
    for resolution in [
        Resolution::SameFile {
            target_symbol: String::new(),
            target_file: String::new(),
        },
        Resolution::ImportScoped {
            target_symbol: String::new(),
            target_file: String::new(),
            imported_from: String::new(),
        },
        Resolution::ReceiverType {
            target_symbol: String::new(),
            target_file: String::new(),
            receiver_type: String::new(),
        },
        Resolution::UniqueGlobal {
            target_symbol: String::new(),
            target_file: String::new(),
            family: LangFamily::Python,
        },
        Resolution::AmbiguousGlobal {
            candidates: Vec::new(),
            family: LangFamily::Python,
        },
        Resolution::Unresolved {
            reason: String::new(),
        },
    ] {
        assert_eq!(
            resolution.confidence(),
            entitled_confidence(&resolution),
            "the crate's own mapping must agree with the table above for {resolution:?}"
        );
    }
    assert!(
        seen_tiers.len() >= 3,
        "the fixture must reach at least three resolution tiers, reached {seen_tiers:?}"
    );
}

/// R-2: a receiver that merely *spells* a type name must not outrank an import.
///
/// `from real import parser` is file-specific evidence about what `parser` is.
/// An unrelated `class parser` in some other file is a spelling coincidence.
/// The literal-type rung sat in rung 1, ahead of the import rungs, so the
/// coincidence won at DETERMINISTIC.
#[test]
fn an_import_binding_outranks_a_same_spelled_type_elsewhere() {
    let result = resolve(&[
        (
            "u.py",
            "from real import parser\n\ndef go():\n    return parser.parse()\n",
        ),
        (
            "real.py",
            "def parser():\n    pass\n\ndef parse():\n    return 1\n",
        ),
        (
            "other.py",
            "class parser:\n    def parse(self):\n        return 2\n",
        ),
    ]);
    assert_eq!(
        calls(&result),
        ["u.py::go->real.py::parse"],
        "the import in this file names what `parser` is; a same-named class \
         elsewhere is a coincidence"
    );

    // Positive control: with no import to contradict it, a receiver written as
    // a literal type name still dispatches — this is `PdgBuilder::new()`.
    let literal = resolve(&[(
        "b.rs",
        "pub struct Builder;\nimpl Builder { pub fn new() -> Self { Builder } }\n\
         pub fn make() -> Builder { Builder::new() }\n",
    )]);
    assert_eq!(
        calls(&literal),
        ["b.rs::make->b.rs::Builder.new"],
        "an associated function on a type declared right here still resolves"
    );
}

/// R-3: a bare call must not bind to a same-file *instance method*.
///
/// `def invoke(): run()` next to `class C: def run(self)` raises `NameError` at
/// runtime. Binding it gives `C.run` a fabricated caller, which also shields it
/// from the dead-code pass.
#[test]
fn a_bare_call_does_not_bind_to_a_same_file_instance_method() {
    let result = resolve(&[(
        "a.py",
        "class C:\n    def run(self):\n        return 1\n\ndef invoke():\n    return run()\n",
    )]);
    assert!(
        !calls(&result)
            .iter()
            .any(|row| row.ends_with("a.py::C.run")),
        "a bare name cannot reach an instance method: {:?}",
        calls(&result)
    );

    // Positive control 1: `self.run()` inside the class does name the sibling.
    let via_self = resolve(&[(
        "b.py",
        "class C:\n    def run(self):\n        return 1\n    def go(self):\n        return self.run()\n",
    )]);
    assert_eq!(
        calls(&via_self),
        ["b.py::C.go->b.py::C.run"],
        "a self-reference still resolves to the sibling method"
    );

    // Positive control 2: a bare call to a same-file *top-level* function
    // resolves exactly as before.
    let top_level = resolve(&[(
        "c.py",
        "def run():\n    return 1\n\ndef invoke():\n    return run()\n",
    )]);
    assert_eq!(calls(&top_level), ["c.py::invoke->c.py::run"]);

    // Positive control 3: languages that supply the receiver implicitly still
    // resolve a bare sibling call. `g()` inside `A::f()` is a real call in C++
    // and Java, and severing it would trade one fabricated edge for thousands
    // of missing ones.
    let cpp = resolve(&[(
        "a.cpp",
        "class A { public: void f() { g(); } void g() {} };\n",
    )]);
    assert_eq!(
        calls(&cpp),
        ["a.cpp::A.f->a.cpp::A.g"],
        "an implicit-receiver sibling call must survive"
    );
    let java = resolve(&[("A.java", "class A { void f() { g(); } void g() {} }\n")]);
    assert_eq!(calls(&java), ["A.java::A.f->A.java::A.g"]);

    // Positive control 4: the cross-file shape the global rung serves for C++ —
    // one class's methods defined across two translation units — still
    // resolves, because the scope test is applied only where the language
    // demands a written receiver. Here the implicit receiver reaches a method
    // whose declaring symbol is in another file, which is precisely what the
    // file-level test would sever.
    let cross_file = resolve(&[
        ("a.cpp", "void A::f() { helper(); }\n"),
        ("b.cpp", "void A::helper() {}\n"),
    ]);
    assert_eq!(
        calls(&cross_file),
        ["a.cpp::A.f->b.cpp::A.helper"],
        "a C++ class split across translation units must keep resolving"
    );
}

/// R-3 (second shape): the global rung obeys the same scope rule.
///
/// Blocking the same-file rung alone only moves the fabricated edge: the
/// bare name reaches the very same instance method through the unique-global
/// rung, at HIGH instead of DETERMINISTIC, and the method is still handed a
/// caller that does not exist and still shielded from the dead-code pass.
#[test]
fn a_bare_call_does_not_reach_an_instance_method_in_another_file_either() {
    let result = resolve(&[
        ("caller.py", "def invoke():\n    return handle()\n"),
        (
            "owner.py",
            "class C:\n    def handle(self):\n        return 1\n",
        ),
    ]);
    assert!(
        calls(&result).is_empty(),
        "a bare name cannot reach another file's instance method: {:?}",
        calls(&result)
    );

    // Positive control: a top-level function in another file still resolves.
    let reachable = resolve(&[
        ("caller.py", "def invoke():\n    return handle()\n"),
        ("owner.py", "def handle():\n    return 1\n"),
    ]);
    assert_eq!(calls(&reachable), ["caller.py::invoke->owner.py::handle"]);
}

/// R-4: a route whose handler did not bind must say so.
///
/// Today "the handler is ambiguous" and "the route has no named handler" are
/// byte-identical: no edge, no record. `HandlesRoute` is what tells liveness a
/// handler is reached from outside the call graph, so both candidates are then
/// reported dead with nothing saying the route was checked and failed.
#[test]
fn a_route_whose_handler_did_not_bind_is_recorded() {
    let route_file = (
        "srv.rs",
        "fn app() -> Router { Router::new().route(\"/items\", get(list_items)) }\n",
    );
    let ambiguous = resolve(&[
        route_file,
        ("h.rs", "pub async fn list_items() {}\n"),
        ("h2.rs", "pub async fn list_items() {}\n"),
    ]);
    let record = ambiguous
        .unresolved
        .iter()
        .find(|entry| entry.callee_name == "list_items")
        .unwrap_or_else(|| {
            panic!(
                "an unbound route handler must be recorded, got {:?}",
                ambiguous.unresolved
            )
        });
    assert_eq!(record.kind, UnresolvedKind::Route);
    assert!(
        record.source_symbol.contains("/items"),
        "the record must name the route it came from, got {:?}",
        record.source_symbol
    );
    let Resolution::Unresolved { reason } = &record.resolution else {
        panic!("a route record carries an Unresolved resolution");
    };
    assert!(
        reason.contains('2'),
        "the record must carry the candidate count so ambiguity is not absence: {reason}"
    );

    // A handler name nothing declares at all is also a failed check.
    let missing = resolve(&[route_file]);
    assert!(
        missing
            .unresolved
            .iter()
            .any(|entry| entry.callee_name == "list_items" && entry.kind == UnresolvedKind::Route),
        "a handler no file declares is a failed bind, not an absent route: {:?}",
        missing.unresolved
    );

    // Positive control 1: a handler that binds produces an edge and no record.
    let unique = resolve(&[route_file, ("h.rs", "pub async fn list_items() {}\n")]);
    assert!(
        unique
            .edges
            .iter()
            .any(|edge| edge.edge_kind == EdgeKind::HandlesRoute),
        "a unique handler still binds"
    );
    assert!(
        !unique
            .unresolved
            .iter()
            .any(|entry| entry.kind == UnresolvedKind::Route),
        "a route that bound must not also be recorded as failed: {:?}",
        unique.unresolved
    );

    // Positive control 2: an anonymous handler is genuinely nameless, so there
    // is nothing to fail to bind and nothing to record.
    let anonymous = resolve(&[("app.js", "app.get('/a', (req, res) => {});\n")]);
    assert!(
        !anonymous
            .unresolved
            .iter()
            .any(|entry| entry.kind == UnresolvedKind::Route),
        "an arrow-function handler has no name to resolve: {:?}",
        anonymous.unresolved
    );
}

/// R-5: an unresolvable *reference* is recorded, exactly as a call is.
///
/// The ledger `devmap build` prints is the R5 completeness ledger. Covering
/// only calls makes it a partial denominator presented as a total.
#[test]
fn an_unresolvable_reference_is_recorded_alongside_the_call() {
    let result = resolve(&[(
        "app.rs",
        "pub fn take(s: Missing) -> Missing { also_missing(); s }\n",
    )]);
    assert!(
        result
            .unresolved
            .iter()
            .any(|entry| entry.callee_name == "also_missing" && entry.kind == UnresolvedKind::Call),
        "the call is recorded today and must stay recorded: {:?}",
        result.unresolved
    );
    assert!(
        result
            .unresolved
            .iter()
            .any(|entry| entry.callee_name == "Missing" && entry.kind == UnresolvedKind::Reference),
        "the unresolvable type reference must be recorded too: {:?}",
        result.unresolved
    );

    // Python parameter annotations are Type references (same as Rust), so an
    // unresolvable one is recorded — with NoNamesake when the corpus has no
    // symbol of that name. The declined rung is a bare Name in *value*
    // position (e.g. reading the parameter `r`), which never reaches the
    // global lookup and therefore never enters the ledger.
    let declined = resolve(&[(
        "app.py",
        "def use(r: Missing):\n    alsoMissing()\n    return r\n",
    )]);
    assert!(
        declined.unresolved.iter().any(|entry| {
            entry.callee_name == "Missing"
                && entry.kind == UnresolvedKind::Reference
                && entry.class == UnresolvedClass::NoNamesake
        }),
        "an unresolvable type annotation must be recorded as NoNamesake: {:?}",
        declined.unresolved
    );
    assert!(
        declined.unresolved.iter().any(|entry| {
            entry.callee_name == "alsoMissing" && entry.kind == UnresolvedKind::Call
        }),
        "the bare call must stay recorded: {:?}",
        declined.unresolved
    );
    assert!(
        declined
            .unresolved
            .iter()
            .all(|entry| entry.callee_name != "r"),
        "reading the parameter `r` is a declined bare Name, not a ledger row: {:?}",
        declined.unresolved
    );

    // Positive control: a reference that *does* resolve is an edge and must not
    // also appear in the ledger.
    let resolved = resolve(&[
        (
            "u.py",
            "from models import Record\n\ndef use(r: Record):\n    return r\n",
        ),
        ("models.py", "class Record:\n    pass\n"),
    ]);
    assert!(
        resolved
            .edges
            .iter()
            .any(|edge| edge.edge_kind == EdgeKind::References),
        "the control fixture must actually resolve its reference"
    );
    assert!(
        !resolved
            .unresolved
            .iter()
            .any(|entry| entry.callee_name == "Record"),
        "a resolved reference must not be reported unresolved: {:?}",
        resolved.unresolved
    );
}

/// R-6: a relative import that resolved to nothing is an index gap, not proof
/// that the name is external.
///
/// `from .helpers import thing` is intra-repository by construction. Labelling
/// it `External` — the tier documented as "demonstrably comes from outside the
/// corpus" and printed as not-worth-acting-on — launders an index gap into the
/// expected bucket.
#[test]
fn an_unindexed_relative_import_is_not_called_external() {
    let gap = resolve(&[(
        "pkg/app.py",
        "from .helpers import thing\n\ndef use():\n    return thing()\n",
    )]);
    let record = gap
        .unresolved
        .iter()
        .find(|entry| entry.callee_name == "thing")
        .unwrap_or_else(|| panic!("the call must be recorded: {:?}", gap.unresolved));
    assert!(
        !matches!(record.class, UnresolvedClass::External { .. }),
        "a relative specifier cannot prove the name is outside the corpus, got {:?}",
        record.class
    );
    assert_eq!(
        record.class,
        UnresolvedClass::Unresolved,
        "an index gap belongs in the tier that means `this may be a defect`"
    );

    // Positive control: a genuinely external module still classifies External.
    let external = resolve(&[(
        "app.py",
        "from requests import get\n\ndef use():\n    return get()\n",
    )]);
    let record = external
        .unresolved
        .iter()
        .find(|entry| entry.callee_name == "get")
        .unwrap_or_else(|| panic!("the call must be recorded: {:?}", external.unresolved));
    assert!(
        matches!(record.class, UnresolvedClass::External { .. }),
        "an absolute specifier that names no indexed file is real evidence, got {:?}",
        record.class
    );
}

/// R-7: an ambiguous call site above the emission ceiling emits no edges —
/// the ledger carries the complete AmbiguousGlobal candidate list instead.
#[test]
fn an_ambiguous_fanout_is_capped_and_says_so() {
    let mut files: Vec<(String, String)> = (0..40)
        .map(|n| {
            (
                format!("d{n}.py"),
                "def spread():\n    return 1\n".to_string(),
            )
        })
        .collect();
    files.push((
        "caller.py".to_string(),
        "def go():\n    return spread()\n".to_string(),
    ));
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, source)| (path.as_str(), source.as_str()))
        .collect();
    let result = resolve(&borrowed);

    let fanout: Vec<_> = result
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls && edge.source_symbol == "caller.py::go")
        .collect();
    assert!(
        fanout.is_empty(),
        "above the ceiling a bare ambiguous call emits zero edges, got {}",
        fanout.len()
    );
    let entry = result
        .unresolved
        .iter()
        .find(|u| u.callee_name == "spread" && u.source_symbol == "caller.py::go")
        .expect("the site must still be in the ledger");
    let Resolution::AmbiguousGlobal { candidates, .. } = &entry.resolution else {
        panic!(
            "ledger row must keep AmbiguousGlobal, got {:?}",
            entry.resolution
        );
    };
    assert_eq!(
        candidates.len(),
        40,
        "the resolution keeps the complete candidate list"
    );

    // Positive control: a fan-out inside the cap still emits every candidate.
    let small = resolve(&[
        ("s0.py", "def few():\n    return 1\n"),
        ("s1.py", "def few():\n    return 1\n"),
        ("c.py", "def go():\n    return few()\n"),
    ]);
    let small_fanout: Vec<_> = small
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls && edge.source_symbol == "c.py::go")
        .collect();
    assert_eq!(
        small_fanout.len(),
        2,
        "an uncapped site keeps every candidate"
    );
    for edge in small_fanout {
        assert_eq!(
            edge.details, None,
            "an in-ceiling fan-out must not claim a truncation"
        );
    }
}

/// R-11: the candidate list must not depend on input slice order.
#[test]
fn ambiguous_candidates_do_not_depend_on_input_order() {
    let files = [
        ("z.py", "def shared():\n    return 1\n"),
        ("m.py", "def shared():\n    return 1\n"),
        ("a.py", "def shared():\n    return 1\n"),
        ("caller.py", "def go():\n    return shared()\n"),
    ];
    let forward = extractions(&files);
    let mut reversed = forward.clone();
    reversed.reverse();

    let candidates_of = |exts: &[Extraction]| -> Vec<Vec<(String, String)>> {
        let mut resolver = Resolver::new();
        resolver.index_extractions(exts);
        resolver
            .resolve_all(exts)
            .edges
            .iter()
            .filter_map(|edge| match edge.resolution.as_deref() {
                Some(Resolution::AmbiguousGlobal { candidates, .. }) => Some(candidates.clone()),
                _ => None,
            })
            .collect()
    };

    let a = candidates_of(&forward);
    let b = candidates_of(&reversed);
    assert!(!a.is_empty(), "the fixture must produce an ambiguous site");
    assert_eq!(
        a, b,
        "candidate lists must be identical under a reversed input slice"
    );
    for list in &a {
        let mut sorted = list.clone();
        sorted.sort();
        assert_eq!(list, &sorted, "candidates are emitted in a total order");
    }
}

/// R-10: a `Resolver` reused for a second snapshot must not resolve Go imports
/// against the module set of the first.
///
/// `index_extractions` documents itself as resetting rebuild state. It cleared
/// twelve fields and left `go_modules` alone, so a second snapshot resolved
/// against stale module prefixes and `replace` directives — at DETERMINISTIC.
#[test]
fn a_second_snapshot_does_not_reuse_the_first_snapshots_go_modules() {
    // `x` is a single-component directory, which the path-suffix tier is too
    // weak to claim: only the module prefix can resolve this import, so the
    // edge's presence is a direct read of whether the module set was used.
    let first = extractions(&[
        (
            "app/main.go",
            "package main\nimport \"example.com/m/x\"\nfunc main() { x.Do() }\n",
        ),
        ("x/s.go", "package x\nfunc Do() {}\n"),
    ]);
    let modules = [GoModule {
        prefix: "example.com/m".to_string(),
        dir: String::new(),
        replaces: Vec::new(),
    }];

    // Positive control: with the modules supplied for this snapshot, the module
    // tier resolves the import.
    let mut resolver = Resolver::new();
    resolver.index_go_modules(&modules);
    resolver.index_extractions(&first);
    let resolved = resolver.resolve_all(&first);
    assert!(
        resolved
            .edges
            .iter()
            .any(|edge| edge.edge_kind == EdgeKind::Imports
                && edge.target_file.contains("package:x/x")),
        "the module prefix must resolve while it is current: {:?}",
        resolved
            .edges
            .iter()
            .filter(|e| e.edge_kind == EdgeKind::Imports)
            .map(|e| e.target_file.clone())
            .collect::<Vec<_>>()
    );

    // A second snapshot, no modules re-supplied: the prefix is no longer known
    // to be current, so it must not be used.
    let second = extractions(&[
        (
            "app/other.go",
            "package main\nimport \"example.com/m/x\"\nfunc other() { x.Do() }\n",
        ),
        ("x/s.go", "package x\nfunc Do() {}\n"),
    ]);
    resolver.index_extractions(&second);
    let stale = resolver.resolve_all(&second);
    let module_tier_hits: Vec<_> = stale
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Imports && edge.source_file == "app/other.go")
        .map(|edge| edge.target_file.clone())
        .collect();
    assert!(
        module_tier_hits.is_empty(),
        "a module set that was not supplied for this snapshot must not be \
         treated as current: {module_tier_hits:?}"
    );
}

/// R-9, retired into a scope assertion (W1.3).
///
/// The original pinned `reexport_chains` as permanently empty. That was true of
/// a field nothing computed, and it is no longer: `compute_reexport_chains`
/// follows `export { x } from './m'` to the file that declares `x`.
///
/// The pin is kept rather than deleted, on the axis it actually protected — the
/// map makes a claim only where the kernel has evidence. Python is the case
/// that tests it: `from .impl import thing` in an `__init__.py` is a re-export
/// in every sense a reader cares about, and the extractor records it as an
/// *import*, carrying no source module on any export. So there is no evidence
/// of the kind this map is built from, and it must stay empty here rather than
/// be filled by inference.
///
/// The ON direction lives in `reexport_chains.rs`, which measures what the
/// chains buy: an `AmbiguousGlobal` fan-out to two files at confidence 0.2
/// collapsing to one `ImportScoped` edge at 1.0.
#[test]
fn reexport_chains_are_only_claimed_where_an_export_names_its_source() {
    let result = resolve(&[
        ("pkg/__init__.py", "from .impl import thing\n"),
        ("pkg/impl.py", "def thing():\n    return 1\n"),
    ]);
    assert!(
        result.reexport_chains.is_empty(),
        "Python export syntax names no source module, so inferring a chain \
         here would be a claim without evidence: {:?}",
        result.reexport_chains
    );
}
