//! W1.3 — a barrel states where a name comes from, and the resolver now reads it.
//!
//! `reexport_chains` shipped documented as "Always empty. Nothing computes
//! this," with R-9 pinning the emptiness as intentional.
//!
//! **The defect that reproduces is not the one the work order predicted.** The
//! prediction was that a symbol consumed only through `index.ts` would have no
//! inbound edge. It does have one: the bare-name global lookup finds it, so a
//! uniquely-named barrel export was never reported dead. Measured on the
//! fixture below.
//!
//! What actually broke is *precision*. When two files declare `thing` and
//! `index.ts` re-exports one of them, the import binding pointed at `index.ts`,
//! which declares no `thing`; the ladder fell through to the global lookup; and
//! the result was an `AmbiguousGlobal` fan-out to **both** files at confidence
//! 0.2 — one of which is an edge to a function the caller demonstrably does not
//! call. The barrel names which one is right, and the resolver was throwing
//! that away.
//!
//! Measured before and after on `an_ambiguous_name_is_disambiguated_by_the_barrel`:
//! two `Calls` edges at 0.2 became one `ImportScoped` edge at 1.0.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::model::{Resolution, ResolutionResult};
use devmap_resolve::Resolver;

fn resolve(files: &[(&str, &str)]) -> (Vec<Extraction>, ResolutionResult) {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    (extractions, resolution)
}

fn call_edges<'a>(
    resolution: &'a ResolutionResult,
    from: &str,
) -> Vec<&'a devmap_resolve::model::ResolvedEdge> {
    resolution
        .edges
        .iter()
        .filter(|e| e.edge_kind == EdgeKind::Calls && e.source_file == from)
        .collect()
}

/// The chain is computed, and it names the file that declares the symbol.
#[test]
fn a_barrel_export_produces_a_chain_to_the_declaring_file() {
    let (_, resolution) = resolve(&[
        (
            "impl.ts",
            "export function thing(): number {\n  return 1;\n}\n",
        ),
        ("index.ts", "export { thing } from './impl';\n"),
    ]);
    assert_eq!(
        resolution.reexport_chains.get("index.ts::thing"),
        Some(&"impl.ts::thing".to_string()),
        "chains: {:?}",
        resolution.reexport_chains
    );
}

/// The defect, and the measurement that proves it was one.
#[test]
fn an_ambiguous_name_is_disambiguated_by_the_barrel() {
    let (_, resolution) = resolve(&[
        (
            "impl.ts",
            "export function thing(): number {\n  return 1;\n}\n",
        ),
        (
            "other.ts",
            "export function thing(): number {\n  return 2;\n}\n",
        ),
        ("index.ts", "export { thing } from './impl';\n"),
        ("app.ts", "import { thing } from './index';\n\nthing();\n"),
    ]);

    let calls = call_edges(&resolution, "app.ts");
    assert_eq!(
        calls.len(),
        1,
        "the barrel names one target, so there is one edge; before this work \
         order there were two, at confidence 0.2, one of them to a function \
         `app.ts` does not call: {calls:?}"
    );
    assert_eq!(calls[0].target_file, "impl.ts");
    assert!(
        !matches!(
            calls[0].resolution.as_deref(),
            Some(Resolution::AmbiguousGlobal { .. })
        ),
        "a stated re-export is not an ambiguity: {:?}",
        calls[0].resolution
    );
    assert!(
        calls[0].confidence.to_millis() > 900,
        "the barrel is evidence, so the edge is deterministic: {:?}",
        calls[0].confidence
    );
}

/// Nested barrels are followed to the terminal file, not one hop.
#[test]
fn a_nested_barrel_resolves_to_the_declaring_file() {
    let (_, resolution) = resolve(&[
        (
            "src/impl.ts",
            "export function deep(): number {\n  return 1;\n}\n",
        ),
        ("src/index.ts", "export { deep } from './impl';\n"),
        ("index.ts", "export { deep } from './src/index';\n"),
    ]);
    assert_eq!(
        resolution.reexport_chains.get("index.ts::deep"),
        Some(&"src/impl.ts::deep".to_string()),
        "a consumer must read one entry, not walk the map: {:?}",
        resolution.reexport_chains
    );
}

/// A renaming re-export asks the target for the *local* name.
///
/// `export { a as b } from './m'` publishes `b` and requests `a`. Keying the
/// lookup on the published name would look for `b` in a module that declares
/// only `a`, and the chain would silently not exist.
#[test]
fn a_renaming_reexport_follows_the_local_name() {
    let (_, resolution) = resolve(&[
        (
            "impl.ts",
            "export function internal(): number {\n  return 1;\n}\n",
        ),
        (
            "index.ts",
            "export { internal as publicName } from './impl';\n",
        ),
    ]);
    assert_eq!(
        resolution.reexport_chains.get("index.ts::publicName"),
        Some(&"impl.ts::internal".to_string()),
        "chains: {:?}",
        resolution.reexport_chains
    );
}

/// The chain was right and the consumer still missed: its binding took the
/// terminal *file* and kept its own name for the symbol, so `publicName()`
/// asked `impl.ts` for a `publicName` it does not declare.
#[test]
fn a_call_through_a_renaming_reexport_reaches_the_declared_name() {
    let (_, resolution) = resolve(&[
        (
            "impl.ts",
            "export function internal(): number {\n  return 1;\n}\n",
        ),
        (
            "index.ts",
            "export { internal as publicName } from './impl';\n",
        ),
        (
            "app.ts",
            "import { publicName } from './index';\n\nexport function run(): number {\n  return publicName();\n}\n",
        ),
    ]);
    let targets: Vec<_> = call_edges(&resolution, "app.ts")
        .into_iter()
        .map(|edge| edge.target_symbol.clone())
        .filter(|target| target.starts_with("impl.ts"))
        .collect();
    assert_eq!(targets, vec!["impl.ts::internal"]);
}

/// A cycle yields no entry, not a guess.
///
/// There is no terminal file, so there is nothing true to record; naming either
/// endpoint would invent an answer the tree does not support. The bound also
/// keeps a malformed barrel from turning into a hang.
#[test]
fn a_cyclic_reexport_produces_no_chain() {
    let (_, resolution) = resolve(&[
        ("a.ts", "export { looped } from './b';\n"),
        ("b.ts", "export { looped } from './a';\n"),
    ]);
    assert!(
        resolution.reexport_chains.is_empty(),
        "a cycle has no terminal file: {:?}",
        resolution.reexport_chains
    );
}

/// A specifier naming no indexed file records no chain.
///
/// That is the same index gap `UnresolvedKind::Import` already reports.
/// Inventing a chain endpoint for it would manufacture graph structure, which
/// is the one thing the unresolved ledger exists to avoid.
#[test]
fn a_reexport_from_outside_the_corpus_records_no_chain() {
    let (_, resolution) = resolve(&[(
        "index.ts",
        "export { createRoot } from 'react-dom/client';\n",
    )]);
    assert!(
        resolution.reexport_chains.is_empty(),
        "chains: {:?}",
        resolution.reexport_chains
    );
}

/// The OFF direction: a plain export is not a chain.
///
/// Without this, a `compute_reexport_chains` that mapped every export to itself
/// would satisfy every assertion above.
#[test]
fn a_direct_export_is_not_a_chain() {
    let (_, resolution) = resolve(&[(
        "impl.ts",
        "export function thing(): number {\n  return 1;\n}\n",
    )]);
    assert!(
        resolution.reexport_chains.is_empty(),
        "`export function` names no source module: {:?}",
        resolution.reexport_chains
    );
}

/// A Python module that re-imports a name passes it through, and the
/// consumer's call reaches the declaration rather than a same-named decoy.
#[test]
fn a_python_reimport_is_followed_to_the_declaring_module() {
    for (barrel_path, barrel, consumer) in [
        (
            "barrel.py",
            "from impl import normalise\n",
            "from barrel import normalise\n\n\ndef main():\n    return normalise(' x ')\n",
        ),
        (
            "barrel.py",
            "from impl import normalise as clean\n",
            "from barrel import clean\n\n\ndef main():\n    return clean(' x ')\n",
        ),
        (
            "pkg/__init__.py",
            "from .impl import normalise\n",
            "from pkg import normalise\n\n\ndef main():\n    return normalise(' x ')\n",
        ),
    ] {
        let impl_path = if barrel_path.starts_with("pkg/") {
            "pkg/impl.py"
        } else {
            "impl.py"
        };
        let (_, resolution) = resolve(&[
            (impl_path, "def normalise(text):\n    return text.strip()\n"),
            (barrel_path, barrel),
            ("legacy.py", "def normalise(text):\n    return text\n"),
            ("app.py", consumer),
        ]);
        let targets: Vec<_> = call_edges(&resolution, "app.py")
            .into_iter()
            .filter(|edge| edge.target_symbol.ends_with("::normalise"))
            .map(|edge| edge.target_symbol.clone())
            .collect();
        assert_eq!(
            targets,
            vec![format!("{impl_path}::normalise")],
            "`{barrel}` passes `normalise` through to the module that declares it"
        );
    }
}

/// R-9, rewritten rather than deleted.
///
/// The original pinned `reexport_chains` as permanently empty, which was a true
/// statement about a field nothing computed. Deleting it would leave the new
/// behaviour unguarded on the axis the pin actually protected: that the map
/// makes claims only where the kernel has evidence. So it becomes a **scope**
/// assertion — chains exist for languages whose exports carry a module
/// specifier, and nowhere else.
///
/// Python was pinned empty here on the reading that a `from .impl import
/// thing` is an *import*, not an export, and so not evidence. That reading was
/// wrong about the language: a name bound at module scope **is** an attribute
/// of the module, so `from pkg import thing` reads `pkg.impl.thing` by
/// Python's own rule, not by inference. Leaving the map empty cost a real
/// finding — the consumer's binding pointed at `__init__.py`, which declares
/// nothing, and the function it re-exports was reported dead at 0.4.
///
/// The evidence boundary moves with the rule and no further: an import inside
/// a function body binds a local, not a module attribute, and makes no chain.
#[test]
fn reexport_chains_are_scoped_to_what_the_language_publishes() {
    let (_, python) = resolve(&[
        ("pkg/__init__.py", "from .impl import thing\n"),
        ("pkg/impl.py", "def thing():\n    return 1\n"),
    ]);
    assert_eq!(
        python.reexport_chains.get("pkg/__init__.py::thing").map(String::as_str),
        Some("pkg/impl.py::thing"),
        "a module-scope import publishes the name as a module attribute: {:?}",
        python.reexport_chains
    );

    let (_, local) = resolve(&[
        (
            "pkg/__init__.py",
            "def lazy():\n    from .impl import thing\n    return thing()\n",
        ),
        ("pkg/impl.py", "def thing():\n    return 1\n"),
    ]);
    assert!(
        local.reexport_chains.is_empty(),
        "an import inside a function binds a local, not a module attribute: {:?}",
        local.reexport_chains
    );

    let (_, typescript) = resolve(&[
        (
            "impl.ts",
            "export function thing(): number {\n  return 1;\n}\n",
        ),
        ("index.ts", "export { thing } from './impl';\n"),
    ]);
    assert!(
        !typescript.reexport_chains.is_empty(),
        "and the language that does carry one must not be empty, or the \
         assertion above holds for the wrong reason"
    );
}
