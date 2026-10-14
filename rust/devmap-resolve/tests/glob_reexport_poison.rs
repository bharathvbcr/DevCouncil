//! A name two globs bring in from two files has no single source — on every
//! round of the glob fixpoint, not just the first — and a glob expansion that
//! stops at its hop cap says so.
//!
//! `expand_glob_hops` runs to a fixpoint because a glob over a module that
//! itself globs only learns the inner names once the inner hops exist. Two
//! defects sat in that loop:
//!
//! * **Only the last glob in a file was ever expanded.** Every glob was stored
//!   in the hop map under `<file>::*`, so `pub use b::*; pub use c::*;` kept
//!   `c` and dropped `b` before the expansion began. The same-round poison rule
//!   could never fire for two globs written in one file, and the file was bound
//!   to `c` alone — one source for a name Rust rejects.
//! * **A hop taken on an earlier round was never reconsidered.** The proposal
//!   loop skipped every key already in the map, so a second glob that only
//!   publishes the name after its own expansion could not poison the first.
//!
//! And the expansion stopped at `REEXPORT_GLOB_MAX_HOPS` with a partial map and
//! nothing on the answer saying the walk had stopped.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::model::ResolutionResult;
use devmap_resolve::Resolver;

fn resolve_with(files: &[(String, String)]) -> (Resolver, ResolutionResult) {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    (resolver, resolution)
}

fn resolve(files: &[(&str, &str)]) -> (Resolver, ResolutionResult) {
    let owned: Vec<(String, String)> = files
        .iter()
        .map(|(path, source)| (path.to_string(), source.to_string()))
        .collect();
    resolve_with(&owned)
}

fn chain<'a>(resolution: &'a ResolutionResult, key: &str) -> Option<&'a str> {
    resolution.reexport_chains.get(key).map(String::as_str)
}

/// Same round: both globs publish `foo` from the start. Written in one file,
/// which is the only place two globs can meet on one key.
#[test]
fn two_globs_in_one_file_publishing_one_name_record_neither() {
    let (_, resolution) = resolve(&[
        ("src/b.rs", "pub fn foo() -> i32 {\n    1\n}\n"),
        ("src/c.rs", "pub fn foo() -> i32 {\n    2\n}\n"),
        (
            "src/lib.rs",
            "pub mod b;\npub mod c;\npub use b::*;\npub use c::*;\n",
        ),
    ]);
    assert_eq!(
        chain(&resolution, "src/lib.rs::foo"),
        None,
        "two globs publish `foo`, so neither file is its source: {:?}",
        resolution.reexport_chains
    );
}

/// The first glob still expands when a second one follows it in the file:
/// a name only one of the two publishes is recorded from that one.
#[test]
fn every_glob_in_a_file_is_expanded_not_only_the_last() {
    let (_, resolution) = resolve(&[
        ("src/b.rs", "pub fn only_b() -> i32 {\n    1\n}\n"),
        ("src/c.rs", "pub fn only_c() -> i32 {\n    2\n}\n"),
        (
            "src/lib.rs",
            "pub mod b;\npub mod c;\npub use b::*;\npub use c::*;\n",
        ),
    ]);
    assert_eq!(
        chain(&resolution, "src/lib.rs::only_b"),
        Some("src/b.rs::only_b"),
        "the earlier glob is expanded too: {:?}",
        resolution.reexport_chains
    );
    assert_eq!(
        chain(&resolution, "src/lib.rs::only_c"),
        Some("src/c.rs::only_c"),
        "{:?}",
        resolution.reexport_chains
    );
}

/// The card's fixture. `lib` globs `b` and `c`; `b` only receives `foo` from
/// its own glob of `d`; `c` declares `foo`. Round one sees `foo` only in `c`
/// and records `lib::foo -> c`. Round two sees `b` publish `foo` too, and that
/// must poison the hop round one took.
#[test]
fn a_glob_that_publishes_a_name_on_a_later_round_poisons_the_earlier_hop() {
    let (_, resolution) = resolve(&[
        ("src/b/d.rs", "pub fn foo() -> i32 {\n    1\n}\n"),
        ("src/b.rs", "pub mod d;\npub use d::*;\n"),
        ("src/c.rs", "pub fn foo() -> i32 {\n    2\n}\n"),
        (
            "src/lib.rs",
            "pub mod b;\npub mod c;\npub use b::*;\npub use c::*;\n",
        ),
    ]);
    assert_eq!(
        chain(&resolution, "src/b.rs::foo"),
        Some("src/b/d.rs::foo"),
        "the inner glob alone is unambiguous: {:?}",
        resolution.reexport_chains
    );
    assert_eq!(
        chain(&resolution, "src/lib.rs::foo"),
        None,
        "`b` and `c` both publish `foo` to `lib`, so neither is its source: {:?}",
        resolution.reexport_chains
    );
}

/// The same fixpoint serves `export *`, and the consumer is where a wrong hop
/// would have done its damage: a confident edge to the file it does not call.
#[test]
fn a_later_round_poison_reaches_the_consumer_in_javascript() {
    let (_, resolution) = resolve(&[
        ("src/d.js", "export function foo() {\n  return 1;\n}\n"),
        ("src/b.js", "export * from './d';\n"),
        ("src/c.js", "export function foo() {\n  return 2;\n}\n"),
        ("src/a.js", "export * from './b';\nexport * from './c';\n"),
        (
            "src/app.js",
            "import { foo } from './a';\nexport function go() {\n  return foo();\n}\n",
        ),
    ]);
    assert_eq!(
        chain(&resolution, "src/a.js::foo"),
        None,
        "{:?}",
        resolution.reexport_chains
    );
    let confident: Vec<&str> = resolution
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls && edge.source_file == "src/app.js")
        .filter(|edge| edge.target_symbol.ends_with("::foo"))
        .filter(|edge| edge.confidence.to_millis() >= 900)
        .map(|edge| edge.target_file.as_str())
        .collect();
    assert!(
        confident.is_empty(),
        "the barrel names no single `foo`, so no edge may claim one: {confident:?}"
    );
}

/// A poisoned hop takes what was built on it with it. `outer` globs `lib`,
/// which on round two learns `foo` from `lib`'s own glob of `c`; once `lib`'s
/// `foo` is poisoned, `outer`'s `foo` has nothing true to point at.
#[test]
fn a_poisoned_hop_leaves_no_chain_in_a_file_that_globbed_it() {
    let (_, resolution) = resolve(&[
        ("src/b/d.rs", "pub fn foo() -> i32 {\n    1\n}\n"),
        ("src/b.rs", "pub mod d;\npub use d::*;\n"),
        ("src/c.rs", "pub fn foo() -> i32 {\n    2\n}\n"),
        (
            "src/mid.rs",
            "pub mod b;\npub mod c;\npub use b::*;\npub use c::*;\n",
        ),
        ("src/lib.rs", "pub mod mid;\npub use mid::*;\n"),
    ]);
    assert_eq!(chain(&resolution, "src/mid.rs::foo"), None);
    assert_eq!(
        chain(&resolution, "src/lib.rs::foo"),
        None,
        "{:?}",
        resolution.reexport_chains
    );
}

/// An explicit re-export still outranks every glob, on every round: the
/// poison rule is for names globs swept in, never for one the author wrote.
#[test]
fn an_explicit_reexport_is_never_poisoned_by_a_later_glob() {
    let (_, resolution) = resolve(&[
        ("src/b/d.rs", "pub fn foo() -> i32 {\n    1\n}\n"),
        ("src/b.rs", "pub mod d;\npub use d::*;\n"),
        ("src/c.rs", "pub fn foo() -> i32 {\n    2\n}\n"),
        (
            "src/lib.rs",
            "pub mod b;\npub mod c;\npub use c::foo;\npub use b::*;\n",
        ),
    ]);
    assert_eq!(
        chain(&resolution, "src/lib.rs::foo"),
        Some("src/c.rs::foo"),
        "{:?}",
        resolution.reexport_chains
    );
}

/// A complete expansion under the cap reports nothing.
#[test]
fn a_complete_glob_expansion_reports_no_cutoff() {
    let (resolver, resolution) = resolve(&[
        ("src/impl.js", "export function thing() {\n  return 1;\n}\n"),
        ("src/index.js", "export * from './impl';\n"),
    ]);
    assert_eq!(
        chain(&resolution, "src/index.js::thing"),
        Some("src/impl.js::thing")
    );
    assert_eq!(resolver.reexport_glob_cutoff(), None);
}

/// One file declaring 1,000 names, globbed by enough files to need more hops
/// than `REEXPORT_GLOB_MAX_HOPS` allows. The map is partial, and the resolver
/// says so with the limit it stopped at.
#[test]
fn a_glob_expansion_stopped_at_the_hop_cap_reports_the_cutoff() {
    const NAMES: usize = 1_000;
    const LIMIT: usize = 200_000;
    let globbers = LIMIT / NAMES + 1;

    let mut implementation = String::new();
    for i in 0..NAMES {
        implementation.push_str(&format!("export function f{i}() {{}}\n"));
    }
    let mut files = vec![("src/impl.js".to_string(), implementation)];
    for g in 0..globbers {
        files.push((
            format!("src/index{g}.js"),
            "export * from './impl';\n".to_string(),
        ));
    }

    let (resolver, resolution) = resolve_with(&files);
    let cutoff = resolver
        .reexport_glob_cutoff()
        .expect("the expansion needed more hops than the cap allows");
    assert_eq!(cutoff.limit, LIMIT);
    assert!(
        cutoff.dropped_at_least > 0,
        "a cutoff names how many hops it left out: {cutoff:?}"
    );
    assert!(
        resolution.reexport_chains.len() <= LIMIT,
        "the map is the partial one: {}",
        resolution.reexport_chains.len()
    );

    // Under the cap, the same shape is complete and reports nothing.
    files.truncate(LIMIT / NAMES);
    let (resolver, resolution) = resolve_with(&files);
    assert_eq!(resolver.reexport_glob_cutoff(), None);
    assert_eq!(
        resolution.reexport_chains.len(),
        (LIMIT / NAMES - 1) * NAMES
    );
}
