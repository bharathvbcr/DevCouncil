//! An adversarial pass over `devmap-resolve`.
//!
//! Resolution is the phase that turns text the extractor was willing to accept
//! into claims about the code. It runs over the *whole* tree on every build
//! (see the note in `resolve_subset`), so anything here whose cost is worse
//! than linear in the input is a cost every build pays, and anything here that
//! is unbounded is unbounded on hostile input from a single file.
//!
//! Two shapes are hunted:
//!
//! * **Bounds** — every loop over parsed text is linear in that text, and every
//!   fan-out is either bounded or deliberately unbounded with the decision
//!   written down. The import ladder walks candidate paths in a `while` loop
//!   over `::`-separated segments, and the extractor's only limit on a
//!   specifier is `MAX_SOURCE_BYTES`.
//! * **Transparency** — a name the resolver could not resolve is recorded, not
//!   dropped, and a name it *did* resolve is echoed verbatim rather than
//!   normalised into something that names no real symbol (Class C).

use std::time::{Duration, Instant};

use devmap_extract::extract_file;
use devmap_extract::model::{
    EdgeKind, ExtractedCall, ExtractedImport, ExtractedSymbol, Extraction, Span, SymbolKind,
};
use devmap_resolve::model::Resolution;
use devmap_resolve::Resolver;

/// Wall-clock ceiling for a single hostile file.
///
/// Not a performance target — a bound. Every case below is O(bytes) work if
/// the implementation is linear, and each input is at most a few hundred KiB,
/// so a correct implementation finishes in milliseconds. Ten seconds
/// distinguishes "slower than ideal" from "does not terminate in any useful
/// sense", which is the only distinction a bound needs to make. Generous
/// because these run in a debug build on a machine shared with other builds.
const HOSTILE_FILE_BUDGET: Duration = Duration::from_secs(10);

fn resolve(extractions: &[Extraction]) -> devmap_resolve::model::ResolutionResult {
    let mut resolver = Resolver::new();
    resolver.index_extractions(extractions);
    resolver.resolve_all(extractions)
}

fn span() -> Span {
    Span {
        start_byte: 0,
        end_byte: 1,
    }
}

/// A hand-built extraction, for shapes no parser will produce on demand.
///
/// Built directly rather than through `extract_file` because the question here
/// is what *resolution* does with the values it is handed. Routing through a
/// grammar would make the test depend on the extractor's own limits, and a
/// grammar that refused the input would make this test pass vacuously.
fn synthetic(
    path: &str,
    language: &str,
    symbols: Vec<ExtractedSymbol>,
    imports: Vec<ExtractedImport>,
    calls: Vec<ExtractedCall>,
) -> Extraction {
    let mut extraction = extract_file(path, "");
    extraction.language = language.to_string();
    extraction.symbols = symbols;
    extraction.imports = imports;
    extraction.calls = calls;
    extraction
}

fn symbol(path: &str, name: &str) -> ExtractedSymbol {
    ExtractedSymbol {
        name: name.to_string(),
        qualified_name: format!("{path}::{name}"),
        kind: SymbolKind::Function,
        span: span(),
        parent_symbol: None,
        signature: None,
        is_exported: true,
        docstring: None,
        body_signature: None,
        declaration_hash: None,
    }
}

fn import(specifier: &str) -> ExtractedImport {
    ExtractedImport {
        raw_import: specifier.to_string(),
        module_specifier: specifier.to_string(),
        imported_names: Vec::new(),
        local_names: Vec::new(),
        alias: None,
        span: span(),
    }
}

fn call(caller: &str, callee: &str) -> ExtractedCall {
    ExtractedCall {
        caller_symbol: Some(caller.to_string()),
        callee_name: callee.to_string(),
        receiver_expr: None,
        span: span(),
    }
}

// ---------------------------------------------------------------------------
// Bounds.
// ---------------------------------------------------------------------------

/// The import ladder's cost is linear in the specifier, not quadratic.
///
/// `resolve_import_path`'s Rust arms walk `parts` from the full path down to a
/// single segment, calling `parts.join("/")` each time. That is
/// `O(segments^2)` characters copied, and the only ceiling on a specifier is
/// the extractor's `MAX_SOURCE_BYTES` — so one `use crate::a::a::…;` line is
/// enough to make a build spend minutes inside a lookup that finds nothing.
/// The same loop exists in the `self::`/`super::` arm.
///
/// The bound must be over the *ladder*, not over the input: refusing long
/// specifiers would be a cap on what can be indexed, and a specifier that is
/// merely long is legitimate.
#[test]
fn a_pathological_import_specifier_does_not_cost_quadratic_time() {
    for (label, prefix) in [
        ("crate", "crate::"),
        ("self", "self::"),
        ("super", "super::"),
    ] {
        // 40k segments: comfortably inside one source file, and 1.6 x 10^9
        // character copies if the join is quadratic.
        let segments = 40_000;
        let specifier = format!("{prefix}{}", vec!["a"; segments].join("::"));
        let extraction = synthetic(
            "src/main.rs",
            "rust",
            vec![symbol("src/main.rs", "main")],
            vec![import(&specifier)],
            Vec::new(),
        );

        let started = Instant::now();
        let result = resolve(std::slice::from_ref(&extraction));
        let elapsed = started.elapsed();
        assert!(
            elapsed < HOSTILE_FILE_BUDGET,
            "a {label}:: specifier with {segments} segments took {elapsed:?}; the \
             candidate ladder rebuilds the whole path on every rung, so its cost \
             is quadratic in a value bounded only by the source-file size limit"
        );
        // Resolving to nothing is the correct answer — no such file exists —
        // but it must be reached, not timed out into.
        assert!(
            !result
                .edges
                .iter()
                .any(|edge| edge.edge_kind == EdgeKind::Imports),
            "a specifier naming no indexed file must not produce an import edge"
        );
    }
}

/// A `super::` chain deeper than the tree does not walk past the root forever.
///
/// Each `super::` calls `parent_dir` on the previous result. `parent_dir`
/// returns `"."` once there is no parent left, and `Path::new(".").parent()` is
/// `Some("")`, which the filter turns back into `"."` — so the walk is
/// idempotent at the root rather than looping. This pins that, because the
/// alternative spelling (returning the input unchanged) would make a deep chain
/// cost one allocation per `super` for no benefit.
#[test]
fn a_super_chain_deeper_than_the_tree_terminates_at_the_root() {
    let specifier = format!("{}target", "super::".repeat(20_000));
    let extraction = synthetic(
        "a/b/c/mod.rs",
        "rust",
        vec![symbol("a/b/c/mod.rs", "here")],
        vec![import(&specifier)],
        Vec::new(),
    );
    let started = Instant::now();
    let result = resolve(std::slice::from_ref(&extraction));
    assert!(
        started.elapsed() < HOSTILE_FILE_BUDGET,
        "a 20k-deep super:: chain took {:?}",
        started.elapsed()
    );
    assert!(result
        .edges
        .iter()
        .all(|edge| edge.edge_kind != EdgeKind::Imports));
}

/// Ambiguity fan-out is quadratic in candidates, and the sort must not make it
/// worse.
///
/// `resolve_subset` emits one edge per (call site x candidate), which PLAN.md
/// §3.1/SC4 records as a deliberate decision — capping it manufactures false
/// dead code. But the final `edges.sort_by` breaks ties with
/// `format!("{:?}", left.resolution)`, and a `Resolution::AmbiguousGlobal`
/// carries its **whole candidate list**. Two edges from the same caller to the
/// same target with the same kind and confidence tie on all six earlier keys,
/// so that `Debug` format runs — serialising every candidate, twice, per
/// comparison.
///
/// That is the shape this pins: the fan-out itself is accepted, the
/// *serialisation of the fan-out inside a comparator* is not. The assertion is
/// on scaling rather than on absolute time, so it says something on any
/// machine: doubling the candidate count must not multiply the cost by far
/// more than two.
#[test]
fn sorting_ambiguous_edges_does_not_serialize_the_candidate_list_per_comparison() {
    fn build(candidates: usize) -> Duration {
        let mut extractions = Vec::new();
        // `candidates` files each declaring the same name, so every call to it
        // is AmbiguousGlobal with that many candidates.
        for i in 0..candidates {
            let path = format!("pkg/c{i}.py");
            extractions.push(synthetic(
                &path,
                "python",
                vec![symbol(&path, "shared")],
                Vec::new(),
                Vec::new(),
            ));
        }
        // One caller making the same ambiguous call repeatedly. Repeats are
        // what create ties on every earlier sort key.
        let caller_calls: Vec<ExtractedCall> = (0..64)
            .map(|_| call("src/caller.py::run", "shared"))
            .collect();
        extractions.push(synthetic(
            "src/caller.py",
            "python",
            vec![symbol("src/caller.py", "run")],
            Vec::new(),
            caller_calls,
        ));

        let started = Instant::now();
        let result = resolve(&extractions);
        let elapsed = started.elapsed();
        assert!(
            result.edges.iter().any(|edge| matches!(
                edge.resolution.as_deref(),
                Some(Resolution::AmbiguousGlobal { .. })
            )),
            "fixture is inert: no ambiguous edges were produced"
        );
        elapsed
    }

    // In-ceiling sizes only: above AMBIGUOUS_FANOUT_CAP sites emit no edges.
    build(4); // warm the code paths and the allocator
    let (ratio, small, large) = (0..3)
        .map(|_| {
            let small = build(8);
            let large = build(16);
            (
                large.as_secs_f64() / small.as_secs_f64().max(1e-6),
                small,
                large,
            )
        })
        .min_by(|left, right| left.0.total_cmp(&right.0))
        .expect("three samples");
    eprintln!("ambiguous-sort scaling: 8 -> {small:?}, 16 -> {large:?}, ratio {ratio:.2}x");

    assert!(
        ratio < 8.0,
        "doubling ambiguous candidates multiplied resolution cost by {ratio:.1}x \
         ({small:?} -> {large:?}); the tie-break key serialises the full \
         candidate list on every comparison"
    );
}

// ---------------------------------------------------------------------------
// Transparency.
// ---------------------------------------------------------------------------

/// A call the ladder cannot resolve is recorded verbatim, whatever it contains.
///
/// R5's rule is that silence is indistinguishable from "there was no call". The
/// hostile question is whether any *particular* callee text can make the record
/// vanish or come back altered — a symbol name that reaches an agent altered is
/// a Class C fabrication regardless of which layer altered it.
#[test]
fn hostile_callee_text_is_recorded_verbatim_and_never_dropped() {
    let hostile = [
        "x<img src=x onerror=alert(1)>",
        "'; DROP TABLE generation_edges; --",
        "\u{202e}esrever",
        "zero\u{200b}width",
        "\u{1f4a5}",
        "a\u{0}b",
        "..%2f..%2fetc%2fpasswd",
        "MATCH \"*\"",
    ];
    let long = "N".repeat(10_000);
    let mut callees: Vec<String> = hostile.iter().map(|s| s.to_string()).collect();
    callees.push(long.clone());

    let calls: Vec<ExtractedCall> = callees
        .iter()
        .map(|callee| call("src/a.py::run", callee))
        .collect();
    let extraction = synthetic(
        "src/a.py",
        "python",
        vec![symbol("src/a.py", "run")],
        Vec::new(),
        calls,
    );
    let result = resolve(std::slice::from_ref(&extraction));

    for callee in &callees {
        let recorded = result
            .unresolved
            .iter()
            .find(|reference| &reference.callee_name == callee);
        let recorded = recorded.unwrap_or_else(|| {
            panic!(
                "an unresolvable call to {:?} was dropped rather than recorded; \
                 silence here is indistinguishable from there having been no call",
                callee.chars().take(40).collect::<String>()
            )
        });
        assert_eq!(
            &recorded.source_symbol, "src/a.py::run",
            "the recorded caller was rewritten"
        );
    }
    assert_eq!(
        result.unresolved.len(),
        callees.len(),
        "the unresolved ledger gained or lost rows"
    );
}

/// A relative import cannot escape the indexed tree.
///
/// `normalize_rel` pops on `..`, and popping an empty stack is a no-op — so
/// `../../../../etc/passwd` normalises to `etc/passwd` rather than to an
/// absolute path outside the repository. That is the behaviour a path stored in
/// the graph depends on: every node path is repo-relative and is later joined
/// to `repo_root` by a query process, so a stored path with a `..` in it is a
/// read outside the repository waiting to happen.
#[test]
fn a_traversing_relative_import_cannot_escape_the_indexed_tree() {
    let victim = synthetic(
        "etc/passwd.ts",
        "typescript",
        vec![symbol("etc/passwd.ts", "secret")],
        Vec::new(),
        Vec::new(),
    );
    for specifier in [
        "../../../../etc/passwd",
        "./../../../../../etc/passwd",
        "..\\..\\..\\etc\\passwd",
    ] {
        let importer = synthetic(
            "src/deep/nested/app.ts",
            "typescript",
            vec![symbol("src/deep/nested/app.ts", "app")],
            vec![import(specifier)],
            Vec::new(),
        );
        let result = resolve(&[victim.clone(), importer]);
        for edge in &result.edges {
            assert!(
                !edge.target_file.starts_with('/') && !edge.target_file.contains(".."),
                "specifier {specifier:?} produced an edge to {:?}, which is not a \
                 repo-relative path",
                edge.target_file
            );
        }
    }
}

/// Resolution is deterministic across two identical runs on hostile input.
///
/// The per-file loop runs under rayon, so a defect that let a candidate list's
/// order depend on scheduling would show up as a graph that differs between
/// builds — and the determinism digest is what the parity harness compares.
/// Hostile names are used because the sort keys are strings, and a comparator
/// that ordered by anything but the bytes would be visible here first.
#[test]
fn hostile_input_resolves_identically_across_runs() {
    let mut extractions = Vec::new();
    for i in 0..40 {
        let path = format!("pkg/\u{202e}m{i}.py");
        extractions.push(synthetic(
            &path,
            "python",
            vec![
                symbol(&path, "shared"),
                symbol(&path, "\u{200b}zero"),
                symbol(&path, &"L".repeat(2_000)),
            ],
            Vec::new(),
            vec![call(&format!("{path}::shared"), "shared")],
        ));
    }

    let digest = |result: &devmap_resolve::model::ResolutionResult| {
        result
            .edges
            .iter()
            .map(|edge| {
                format!(
                    "{}|{}|{}|{}|{:?}|{}",
                    edge.source_file,
                    edge.source_symbol,
                    edge.target_file,
                    edge.target_symbol,
                    edge.edge_kind,
                    edge.confidence.0
                )
            })
            .collect::<Vec<_>>()
    };

    let first = digest(&resolve(&extractions));
    let second = digest(&resolve(&extractions));
    assert_eq!(
        first, second,
        "two identical resolutions of the same hostile input disagreed"
    );
    assert!(!first.is_empty(), "fixture is inert: no edges produced");
}

/// An ambiguous call above the emission ceiling keeps every candidate on the
/// ledger and emits no edges. Every candidate still names a real indexed file.
///
/// SC4 records the decision not to *collapse* the fan-out — not to pick a
/// winner and hide the ambiguity. Above [`AMBIGUOUS_FANOUT_CAP`] that means
/// one ledger row with the complete candidate list, not a capped sample of
/// edges that would still fabricate inbound callers.
#[test]
fn every_ambiguous_candidate_names_an_indexed_file() {
    let mut extractions = Vec::new();
    let mut indexed = std::collections::BTreeSet::new();
    for i in 0..64 {
        let path = format!("pkg/m{i}.py");
        indexed.insert(path.clone());
        extractions.push(synthetic(
            &path,
            "python",
            vec![symbol(&path, "shared")],
            Vec::new(),
            Vec::new(),
        ));
    }
    indexed.insert("src/caller.py".to_string());
    extractions.push(synthetic(
        "src/caller.py",
        "python",
        vec![symbol("src/caller.py", "run")],
        Vec::new(),
        vec![call("src/caller.py::run", "shared")],
    ));

    let result = resolve(&extractions);
    let ambiguous_edges: Vec<_> = result
        .edges
        .iter()
        .filter(|edge| {
            matches!(
                edge.resolution.as_deref(),
                Some(Resolution::AmbiguousGlobal { .. })
            )
        })
        .collect();
    assert!(
        ambiguous_edges.is_empty(),
        "above the ceiling AmbiguousGlobal emits no edges; got {}",
        ambiguous_edges.len()
    );

    let entry = result
        .unresolved
        .iter()
        .find(|u| u.callee_name == "shared")
        .expect("the site must still be in the ledger");
    let Resolution::AmbiguousGlobal { candidates, .. } = &entry.resolution else {
        panic!("ledger keeps AmbiguousGlobal, got {:?}", entry.resolution);
    };
    assert_eq!(
        candidates.len(),
        64,
        "the resolution must keep every candidate"
    );
    for (file, _) in candidates {
        assert!(
            indexed.contains(file),
            "a kept candidate names {file:?}, which was never indexed"
        );
    }

    // Positive control: inside the ceiling, every candidate is still an edge.
    let mut small = Vec::new();
    for i in 0..2 {
        let path = format!("s{i}.py");
        small.push(synthetic(
            &path,
            "python",
            vec![symbol(&path, "few")],
            Vec::new(),
            Vec::new(),
        ));
    }
    small.push(synthetic(
        "c.py",
        "python",
        vec![symbol("c.py", "go")],
        Vec::new(),
        vec![call("c.py::go", "few")],
    ));
    let inside = resolve(&small);
    let edges: Vec<_> = inside
        .edges
        .iter()
        .filter(|e| {
            matches!(
                e.resolution.as_deref(),
                Some(Resolution::AmbiguousGlobal { .. })
            )
        })
        .collect();
    assert_eq!(edges.len(), 2, "in-ceiling AmbiguousGlobal still emits");
}
