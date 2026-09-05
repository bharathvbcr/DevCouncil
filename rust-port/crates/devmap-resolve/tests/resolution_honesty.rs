//! Nine shapes where a resolver is tempted to claim more than it knows.
//!
//! The ladder's tiers are evidence (`Resolution::confidence`), and every one of
//! these fixtures is a case where the wrong rung would look right: a bare name
//! that a method of the same name could absorb, an import that shadows a
//! builtin, two declarations of one class in one package, a re-export chain
//! whose middle file declares nothing, a route reached only through a
//! decorator, a receiver whose type is written two statements above the call
//! and then overwritten, `self.method()` declared only by a parent, a Go
//! interface method against a concrete method of the same name, and a
//! TypeScript default export renamed at the import.
//!
//! Four invariants are asserted over the whole corpus rather than per shape,
//! because the failure this hunts is an edge that is *individually* plausible:
//!
//! 1. Every emitted edge's `confidence` is exactly what its `Resolution`
//!    entitles it to — `Resolution::confidence` is the only mapping, and this
//!    re-derives it independently rather than calling it.
//! 2. No `DETERMINISTIC` edge without deterministic evidence: the declaration
//!    is in this very file, an import in this file names it, or the receiver's
//!    type is known and declares exactly one method of that name.
//! 3. Ambiguity never picks a winner. Where several files declare one name and
//!    nothing ties the call to any of them, the answer is the fan-out at
//!    `SPECULATIVE` or no edge — never one confident edge.
//! 4. A `DETERMINISTIC` edge names a target the *evidence* names: an
//!    `ImportScoped` edge must point at the file the import resolved to, and a
//!    `SameFile` edge must point at the calling file.

use devmap_extract::extract_file;
use devmap_extract::model::{Confidence, EdgeKind, Extraction};
use devmap_resolve::model::{Resolution, ResolutionResult};
use devmap_resolve::Resolver;

fn resolve(files: &[(&str, &str)]) -> (Vec<Extraction>, ResolutionResult) {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    (extractions, resolution)
}

/// The confidence each rung is entitled to, written out here rather than
/// obtained from `Resolution::confidence`. A test that asks the owner what the
/// answer is cannot catch the owner changing it.
fn entitled(resolution: &Resolution) -> Confidence {
    match resolution {
        Resolution::SameFile { .. }
        | Resolution::ImportScoped { .. }
        | Resolution::ReceiverType { .. }
        | Resolution::Structural { .. } => Confidence::DETERMINISTIC,
        Resolution::UniqueGlobal { .. } => Confidence::HIGH,
        Resolution::AmbiguousGlobal { .. } | Resolution::Unresolved { .. } => {
            Confidence::SPECULATIVE
        }
    }
}

/// A bare call that a same-named instance method could absorb.
const BARE_VS_METHOD: &[(&str, &str)] = &[
    (
        "svc.py",
        "class Service:\n    def process(self, rows):\n        return rows\n\n\n\
         def process(rows):\n    return rows\n",
    ),
    (
        "bare_caller.py",
        "from svc import Service\n\n\ndef run(rows):\n    return process(rows)\n",
    ),
];

/// An import that shadows a builtin. The import is a fact the author wrote, so
/// the call belongs to `shadowlib.len` — not to the builtin, and not nowhere.
const SHADOWED_BUILTIN: &[(&str, &str)] = &[
    ("shadowlib.py", "def len(rows):\n    return 0\n"),
    (
        "shadow_user.py",
        "from shadowlib import len\n\n\ndef count(rows):\n    return len(rows)\n",
    ),
];

/// Two files declaring the same class in one package.
const DUPLICATE_CLASS: &[(&str, &str)] = &[
    ("pkg/first.py", "class Widget:\n    pass\n"),
    ("pkg/second.py", "class Widget:\n    pass\n"),
    ("pkg/use.py", "def make():\n    return Widget()\n"),
];

/// A re-export chain: the file the import names declares nothing itself.
const REEXPORT_CHAIN: &[(&str, &str)] = &[
    ("chain/deep.py", "def target():\n    return 1\n"),
    ("chain/mid.py", "from chain.deep import target\n"),
    ("chain/top.py", "from chain.mid import target\n"),
    (
        "chain/app.py",
        "from chain.top import target\n\n\ndef go():\n    return target()\n",
    ),
];

/// A route reached only through a decorator.
const DECORATED_ROUTE: &[(&str, &str)] = &[
    (
        "web/routes.py",
        "from web.framework import app\n\n\n@app.route(\"/x\")\ndef handler():\n    return 1\n",
    ),
    (
        "web/framework.py",
        "class App:\n    def route(self, path):\n        return path\n\n\napp = App()\n",
    ),
];

/// A receiver whose type is written two statements above the call — and, in the
/// second function, overwritten before it. Both types declare `start`.
const RECEIVER_FROM_ASSIGNMENT: &[(&str, &str)] = &[
    (
        "recv/engines.py",
        "class Engine:\n    def start(self):\n        return 1\n\n\n\
         class Other:\n    def start(self):\n        return 2\n",
    ),
    (
        "recv/drive.py",
        "from recv.engines import Engine, Other\n\n\n\
         def go():\n    e = Engine()\n    n = 0\n    return e.start()\n\n\n\
         def reassigned():\n    e = Engine()\n    e = Other()\n    return e.start()\n",
    ),
];

/// `self.helper()` in a subclass whose parent declares the method.
const INHERITED_SELF_CALL: &[(&str, &str)] = &[
    (
        "inherit/base.py",
        "class Base:\n    def helper(self):\n        return 1\n",
    ),
    (
        "inherit/child.py",
        "from inherit.base import Base\n\n\nclass Child(Base):\n    def run(self):\n        \
         return self.helper()\n",
    ),
];

/// A Go interface method against a concrete method of the same name.
const GO_INTERFACE_METHOD: &[(&str, &str)] = &[
    (
        "geo/shape.go",
        "package geo\n\ntype Shape interface {\n\tArea() float64\n}\n\n\
         type Square struct{ side float64 }\n\n\
         func (s Square) Area() float64 { return s.side * s.side }\n",
    ),
    (
        "geo/measure.go",
        "package geo\n\nfunc Measure(s Shape) float64 { return s.Area() }\n",
    ),
];

/// A TypeScript default export renamed at the import.
const TS_DEFAULT_RENAMED: &[(&str, &str)] = &[
    (
        "ts/widget.ts",
        "export default function widget() {\n  return 1;\n}\n",
    ),
    (
        "ts/app.ts",
        "import renamed from \"./widget\";\n\nexport function go() {\n  return renamed();\n}\n",
    ),
];

fn whole_corpus() -> Vec<(&'static str, &'static str)> {
    let mut files = Vec::new();
    for group in [
        BARE_VS_METHOD,
        SHADOWED_BUILTIN,
        DUPLICATE_CLASS,
        REEXPORT_CHAIN,
        DECORATED_ROUTE,
        RECEIVER_FROM_ASSIGNMENT,
        INHERITED_SELF_CALL,
        GO_INTERFACE_METHOD,
        TS_DEFAULT_RENAMED,
    ] {
        files.extend_from_slice(group);
    }
    files
}

#[test]
fn every_edge_over_the_hard_shapes_claims_only_what_its_rung_entitles_it_to() {
    let (_, result) = resolve(&whole_corpus());
    assert!(
        result.edges.len() > 20,
        "the fixture must actually exercise the ladder, got {} edges",
        result.edges.len()
    );
    for edge in &result.edges {
        let resolution = edge
            .resolution
            .as_deref()
            .expect("every emitted edge names its evidence");
        assert_eq!(
            edge.confidence,
            entitled(resolution),
            "{} -> {} ({:?}) claims {:?} on evidence {:?}",
            edge.source_symbol,
            edge.target_symbol,
            edge.edge_kind,
            edge.confidence,
            resolution
        );
    }
}

/// The one-directional statement of the same rule, so a resolution variant
/// added later cannot quietly join the deterministic tier.
#[test]
fn no_deterministic_edge_exists_without_deterministic_evidence() {
    let (_, result) = resolve(&whole_corpus());
    for edge in &result.edges {
        if edge.confidence != Confidence::DETERMINISTIC {
            continue;
        }
        let resolution = edge.resolution.as_deref().expect("evidence");
        assert!(
            matches!(
                resolution,
                Resolution::SameFile { .. }
                    | Resolution::ImportScoped { .. }
                    | Resolution::ReceiverType { .. }
                    | Resolution::Structural { .. }
            ),
            "a certainty was claimed on {resolution:?} for {} -> {}",
            edge.source_symbol,
            edge.target_symbol
        );
    }
}

/// A deterministic edge must point where its evidence points. `SameFile` that
/// names another file, or `ImportScoped` that names a file the import did not
/// resolve to, is the tier being right about the rung and wrong about the
/// target — which reads to every consumer as a certainty.
#[test]
fn a_deterministic_edge_names_the_target_its_evidence_names() {
    let (_, result) = resolve(&whole_corpus());
    for edge in &result.edges {
        match edge.resolution.as_deref() {
            Some(Resolution::SameFile { target_file, .. }) => assert_eq!(
                (&edge.target_file, &edge.source_file),
                (target_file, target_file),
                "a SameFile edge that leaves its file: {edge:?}"
            ),
            Some(Resolution::ImportScoped { target_file, .. }) => assert_eq!(
                &edge.target_file, target_file,
                "an ImportScoped edge pointing away from the import's target: {edge:?}"
            ),
            Some(Resolution::ReceiverType { target_file, .. }) => assert_eq!(
                &edge.target_file, target_file,
                "a ReceiverType edge pointing away from the receiver's type: {edge:?}"
            ),
            // Not a resolved reference: the synthetic package node a Go file
            // declares itself a member of. It still has to point where its
            // evidence points.
            Some(Resolution::Structural { target_file, .. }) => assert_eq!(
                &edge.target_file, target_file,
                "a structural edge pointing away from the node it names: {edge:?}"
            ),
            _ => {}
        }
    }
}

/// Two files declare `Widget` and nothing ties `pkg/use.py` to either. The
/// honest answers are the fan-out at `SPECULATIVE` or no edge at all; the one
/// answer that must never appear is a single confident edge to one of them.
#[test]
fn ambiguity_never_picks_a_winner() {
    let (_, result) = resolve(DUPLICATE_CLASS);
    let picks: Vec<_> = result
        .edges
        .iter()
        .filter(|edge| edge.source_file == "pkg/use.py" && edge.target_symbol.contains("Widget"))
        .collect();
    for edge in &picks {
        assert_eq!(
            edge.confidence,
            Confidence::SPECULATIVE,
            "one of two equally-matching declarations was chosen with confidence: {edge:?}"
        );
        assert!(
            matches!(
                edge.resolution.as_deref(),
                Some(Resolution::AmbiguousGlobal { .. })
            ),
            "a pick among equals must name itself ambiguous: {edge:?}"
        );
    }
    if picks.len() == 1 {
        panic!(
            "a single edge for a name two files declare is a winner picked in \
             all but name: {:?}",
            picks[0]
        );
    }
}

/// The import is a fact the author wrote, so a name that shadows a builtin
/// belongs to the imported module. Dropping it would leave `shadowlib.len`
/// looking uncalled.
#[test]
fn an_import_that_shadows_a_builtin_still_binds_to_the_import() {
    let (_, result) = resolve(SHADOWED_BUILTIN);
    let edge = result
        .edges
        .iter()
        .find(|edge| {
            edge.source_file == "shadow_user.py"
                && edge.edge_kind == EdgeKind::Calls
                && edge.target_file == "shadowlib.py"
        })
        .unwrap_or_else(|| {
            panic!(
                "the shadowed call resolved to nothing: {:?}",
                result
                    .edges
                    .iter()
                    .filter(|edge| edge.source_file == "shadow_user.py")
                    .collect::<Vec<_>>()
            )
        });
    assert_eq!(
        edge.confidence,
        Confidence::DETERMINISTIC,
        "an import statement in this file naming the target is a fact: {edge:?}"
    );
}
